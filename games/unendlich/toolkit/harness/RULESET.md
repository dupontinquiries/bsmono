# unendlich — harness RULESET

The contract every change (assistant's or yours) is checked against. Two layers:

- **Static layer** — `toolkit/harness/validate.py`, pure Python, no compiler.
  Runs anywhere (including the assistant's sandbox). Catches structural /
  contract / footgun errors. ERROR ⇒ exit 1; WARN ⇒ eyeball.
- **Real layer** — `toolkit/harness/run.sh`, which also invokes the real tools
  when present: `glslc`/`glslangValidator` (GLSL→SPIR-V) and `cargo test`
  (the `vk-harness` suite + engine unit tests). This is the authoritative
  validation and runs on your machine.

The point: the assistant runs the static layer before shipping so it stops
shipping blind; you run `run.sh` (static + real) to validate and to give the
assistant a tight iterate loop.

---

## §winding — CPU mesher is wound INWARD on purpose (v0.24.90 lesson)

The render pipeline is `front_face = CLOCKWISE` + cull BACK. Under the
projection's Y-flip, the **CPU greedy mesher emits INWARD math-normals and that
is what renders correctly** (inward = visible from outside). This was proven the
hard way: v0.24.90 added a global `ChunkMesh::flip_winding()` to "fix" it, which
back-faced all the previously-visible geometry (grass tops, stone). It was
reverted.

RULES:
- `render/mesher.rs` must **not** contain `flip_winding` (static check enforces).
- The test `greedy_all_face_normals_face_outward_under_pipeline` must keep
  asserting `dot < 0` (inward). Do not flip it to `dot > 0`.
- Do not flip `chunk_pipeline.rs`'s `front_face` (it is shared with the GPU
  octree mesher, which is independently correct).
- The GPU octree mesher (`chunk_mesh_octree.comp`) and the CPU mesher render
  correctly under **opposite** effective winding — treat them as independent;
  do not "make one match the other."

## §bindings — descriptor sets must be consistent and unique

- No duplicate `(set, binding)` within a shader (static check enforces).
- `chunk.frag` uses exactly bindings `{0,1,2,3}` (camera UBO, shadow sampler,
  froxel volume, atlas). Adding/removing one requires updating BOTH the shader
  and `world/shader_contract.rs` (and `world_renderer.rs` descriptor writes).
- The face-UV reconstruction expressions in `chunk.frag` must match
  `world/texture_mapping.rs::face_uv` (static check + `shader_contract.rs`).
- Push-constant blocks should stay ≤128 bytes unless the device limit is known
  larger (static check WARNs above 128).

## §spv — never commit generated SPIR-V

`build.rs` compiles GLSL→`.spv` with a SHA-256 hash sidecar. The `.spv` /
`.spv.hash` files are build artifacts. **Never stage them** (static check
enforces on `git diff --cached`). Edit GLSL only; the build regenerates SPV.

## §froxel — volumetric fog is LIVE

`froxel_fill.comp` fills the `R16G16B16A16_SFLOAT` 160×90×64 volume every frame;
`chunk.frag` samples it (binding 2) and composites. It is wired and running —
not "groundwork". Don't re-stub it.

## §ssr — post-FX pipeline + screen-space reflections contract

Built incrementally and **gated behind `UNENDLICH_POSTFX`** (default off ⇒ the
existing direct-to-swapchain path is byte-identical, so the new path is opt-in
and A/B-able). `UNENDLICH_SSR` / `UNENDLICH_BLOOM` are sub-toggles that require
`POSTFX`. Each increment must pass `run.sh` before ship.

Gated path (when `POSTFX=1`): the chunk/entity pipelines are created against an
**offscreen** render pass whose color attachment is the HDR target; the scene
renders there; a fullscreen **present pass** (`present.vert`/`present.frag`)
samples it, tonemaps, and writes the swapchain; egui moves to the present pass
(post-tonemap). When off, everything renders straight to the swapchain as today.

Resource contract:
- Offscreen color: `R16G16B16A16_SFLOAT`, usage `COLOR_ATTACHMENT | SAMPLED`,
  final layout `SHADER_READ_ONLY_OPTIMAL` after the offscreen pass.
- Depth (`render/depth.rs`): gains `SAMPLED` usage + a sampler; `pick_depth_format`
  must require `SAMPLED_IMAGE | DEPTH_STENCIL_ATTACHMENT`.
- Present pass descriptors: `set 0, binding 0 = sampler2D scene_hdr`
  (binding 1 = `sampler2D scene_depth` added in increment 3 for the march).
- Tonemap location: when `POSTFX` is on, `chunk.frag` skips its in-shader
  tonemap/grade (outputs linear HDR) via a UBO flag; `present.frag` applies the
  identical chain. The three grade consts must stay in sync between the two.

Increments:
- **1 — offscreen foundation.** Offscreen HDR + sampleable depth + present pass +
  egui moved + conditional tonemap. ACCEPTANCE: `POSTFX=1` looks **identical** to
  `POSTFX=0` (proves the plumbing with no effect math). No CPU-ref test (it's
  plumbing); validated by the side-by-side look + `run.sh`.
- **2 — reconstruction.** `present.frag` reconstructs view-space position +
  normal from depth (no normal G-buffer). Math lives in pure functions with a
  CPU-reference test `crates/vk-harness/tests/ssr_reconstruct.rs` (pattern of
  `gpu_meshing_octree_cpu_ref.rs`): round-trips a known view-pos → uv+depth →
  reconstruct, and a known-plane normal — **no GPU**, runs in `cargo test`.
- **3 — ray-march + composite.** Screen-space march with a reflectivity mask
  (water = reflective). CPU-reference test `ssr_raymarch.rs` validates march
  hit/miss + reflected-uv on synthetic depth. Reflection is fresnel-blended,
  composited into the linear color BEFORE the present-pass tonemap, only where
  the mask says reflective.

Bloom rides the same offscreen target: bright-pass → `ui_downsample`/`ui_blur`
chain → additive composite in `present.frag` before tonemap. It is the cheapest
visible payoff once increment 1 lands.

Every increment: new descriptor bindings unique + mirrored in Rust; new shaders
pass static + real compile; CPU-ref tests pass; `POSTFX=0` stays identical.

---

## §water — wave field is one shared, physics-driven, CPU-mirrorable function

The water surface waves live in `shaders/water_waves.glsl` and are a PURE
function of `(world XZ, time, wind vector + magnitude, temperature)` — no
textures, no GPU-only state. Rules:

- **Single source of truth.** The CPU mirror is `world/water_waves.rs` — it
  transcribes the SAME math: same constants (`WW_*`), same per-octave phase
  offset, same column-major rotation (`mat2(c,s,-s,c)`), same squared-sine
  waveform, same swell. Its tests lock determinism, that the analytic gradient
  matches a finite difference (validating the normal optimization), the
  physics response, and the swell. Permitted divergence: last-ULP GPU/CPU
  `sin`/`cos`/`sqrt` (sub-micron for height, irrelevant to buoyancy). When
  buoyancy/entity-collision lands it calls `water_waves.rs`; keep the two files
  edited together.
- **Physics, not a generic FBM.** Wind direction sets propagation direction;
  wind magnitude sets amplitude + speed (calm↔chop); temperature damps toward
  glassy as water nears the freeze band. Humidity / weather / season / an
  entity-ripple displacement term are documented future inputs to the same
  `WaveParams` — add them there, not as ad-hoc shader code.
- **Normal in one eval.** The surface normal comes from the analytic gradient
  returned alongside the height (`water_wave_height_grad`), NOT a finite
  difference — keep it that way (≈3x cheaper) and keep the grazing-angle damping
  (kills shimmer + specular aliasing).
- **The wave normal is the SSR input.** When SSR lands it must reflect off this
  perturbed normal so reflections churn instead of staying mirror-sharp; do not
  feed SSR the flat face normal for water.
- `WATER_WAVE_NORMALS` (const in the include) is the A/B toggle; flipping it must
  fall back to the flat-normal surface with no other change.

---

## §bloom — additive HDR bloom, gated under post-FX

Bloom (`UNENDLICH_BLOOM`) is a SUB-toggle of post-FX (`UNENDLICH_POSTFX`): it
samples the linear-HDR scene offscreen, so it only constructs when post-FX is
active. Rules:

- **Off must equal 1c.** With `UNENDLICH_POSTFX=1` but bloom OFF, the result is
  byte-identical to the v0.24.103 tonemap-move. Mechanism: present binding 1 is
  always a valid image (the bloom target when on, the scene image itself when
  off) and the `bloom_intensity` push constant is 0 when off, so the additive
  composite (`color += texture(bloom, uv) * intensity`) is an exact no-op. No
  bloom render pass is recorded when off.
- **Composite is LINEAR and pre-tonemap.** Bloom is added in `present.frag`
  BEFORE `tonemap_grade` (same site SSR will use). Adding after the tonemap
  would double-grade the glow.
- **Fragment passes, not compute.** Bloom reuses the proven offscreen-target +
  sampling-render-pass + fullscreen-triangle pattern (`OffscreenTarget`,
  `create_bloom_render_pass`, a `PresentPipeline`-shaped pipeline). The bloom
  render pass' final layout is SHADER_READ_ONLY, so the present sampler reads it
  with no hand-rolled image barrier; its two external dependencies make a
  ping-pong target safe across frames.
- **Bright-pass = soft-knee threshold at half-res, then a separable blur.**
  `bloom_prefilter.frag` does a 4-tap box downsample (the ui_downsample trick) +
  a Karis/COD soft-knee threshold (push: `threshold`, `knee`) into a half-res
  `R16G16B16A16_SFLOAT` target A. `bloom_blur.frag` is then run ping-ponging A<->B
  (H: A->B, V: B->A), `BLOOM_BLUR_PASSES` times — a 9-tap HDR Gaussian (the
  ui_blur kernel, but writing float not rgba8). `BLOOM_BLUR_PASSES = 0` is a
  valid A/B fallback to the raw bright-pass. The blur reuses the SAME bloom
  render pass (its WAR/RAW external deps make the A<->B ping-pong safe).
- **Shader registration.** `bloom_prefilter.frag` is compiled by the same dev
  shader path as `present.vert`/`present.frag` (whole `shaders/` dir), not the
  build.rs explicit list — mirror that precedent; do not special-case it.

---

## How to run

Assistant (sandbox, static only):
```
python3 toolkit/harness/validate.py
```

You (full validation):
```
bash toolkit/harness/run.sh            # static + real compile + cargo tests
bash toolkit/harness/run.sh --quick    # static + real compile only (skip cargo)
```

Exit non-zero ⇒ something failed; the output names the rule.
