# Neural Shading Integration — wiring the r301 forward-port into r300's deferred path

> Status: design (r301). The neural deferred shader (validated end-to-end in the
> Python parallel-engine: MLP learns 22-raycast AO + soft-shadow from a G-buffer +
> 6 hint rays, ~28–31 dB, on bit-exact Q8 Vulkan kernels) is **forward-ported into
> r300 and compiling** (`render/q8.rs`, `render/neural_shade_pipeline.rs`,
> `shaders/neural_feat_octree.comp`, `shaders/neural_shade.comp`) — but **inert**:
> nothing constructs or dispatches it. This doc is the recipe to make it *run*,
> grounded in r300's actual render code, with the prototype's `INTEGRATION_BUGWATCH`
> folded in as the runtime checklist so the silent-wrongness traps are pre-empted.
> Cross-links: `STRUCTURES_render.md` (render pipeline), `shader_pass_notes.md`,
> `light_field_architecture.md` (shared substrate — the hint rays are occlusion
> queries, same as mob LOS; the G-buffer is what the light field reads against),
> `dynamic_world_architecture.md` (player vision = the highest-fidelity consumer).
>
> **The decision gate is not in this doc.** Whether neural shading is a perf *win*
> is settled only by `bench_lean.py` on the M2 — lavapipe shows 0.25× because it's
> GEMM-bound on a CPU and the ratios don't transfer; on-device the MLP collapses
> toward its FLOP ceiling and the 22→6 ray reduction becomes the win. Run that
> first; everything below assumes it clears.

---

## 0. What's done vs. what this specs

**Done (r301, compile-verified in the sandbox):** the forward-port. `q8.rs`
(quantizer, byte-identical to the Python oracle), `neural_shade_pipeline.rs` (the
**fused-MLP reference** pipeline, ash, in *harness* form — own submit + host IO),
and both `.comp` shaders compiling against r300's real `octree_sample.glsl`. All deps
intact (octree format, `OctreeSlot::base_node_index()`, `ash 0.37`).

**This doc specs the wiring** — all of it needs the live render context, so it's
M2-side and runtime-validated, not sandbox-checkable. A few *pieces* are
sandbox-compilable (noted in §10) and can land as follow-up releases.

---

## 1. Architecture: deferred (the only path temporal is cheap on)

```
chunk pass (MRT) ──► G-buffer {normal, albedo, worldpos, depth}
                          │
                  neural feature pass (compute)  ── octree + base table + 6 hint rays
                          │  → feature buffer (device-local)
                  neural shade pass (compute)    ── Q8 MLP (GEMM, see §4)
                          │  → HDR target
                  [temporal accumulate]          ── reuse temporal_accumulate.rs (§7)
                          │
                  postfx (bloom/exposure) ──► present
```

r300 is forward-rendered (`chunk.frag` → single `out_color`). Neural shading is a
deferred compute pass over a G-buffer; that's a backend addition, and it's the path
the validated prototype used and the only one that makes temporal reuse affordable.

---

## 2. The G-buffer producer (`chunk.frag` MRT) + attachments

`chunk.frag` today: `layout(location=0) out vec4 out_color;`, and it already has
`v_world_pos` (in, location 1) and computes the face normal and base albedo. Add
three outputs:

```glsl
layout(location = 1) out vec4 out_gnormal;    // axis-snapped face normal, [0,1] encoded
layout(location = 2) out vec4 out_galbedo;    // base albedo (r300: continuous v_color*detail)
layout(location = 3) out vec4 out_gworldpos;  // world pos — emit v_world_pos DIRECTLY
```

- **`out_gworldpos` is the load-bearing channel.** BUGWATCH B1: worldpos jitter
  σ=0.3 voxel costs **−3.2 dB** (σ=0.6 → craters), because it's the hint-ray origin.
  Emit `v_world_pos` directly; **never reconstruct from depth** — reconstruction
  error lands exactly on the most sensitive channel. It tolerates no fp16 here; the
  others do (normal → octahedral/fp16 fine, hint distances → fp16 free).
- **`out_galbedo` must match the trainer.** BUGWATCH B2: the prototype trained on a
  discrete palette; r300's albedo is continuous (`v_color*detail`). Two one-liners:
  read albedo from `out_galbedo` in the feature pass, **and retrain** the net with
  albedo sampled the same way before `q8::quantize_layer`. Net architecture unchanged.
- **Attachments:** add the 3 color attachments to the offscreen chunk render pass
  (`offscreen.rs`) and extend the chunk pipeline's color-blend state
  (`chunk_renderer.rs`) to 4 attachments. Depth already exists.
- **Opaque mask:** gate the neural shade to opaque blocks; water/clouds keep their
  forward shaders (they're volumes/transparent — §9). A material bit in the G-buffer
  or a stencil.

---

## 3. The feature pass (`NeuralFeatPipeline`)

Copy `neural_shade_pipeline.rs`'s structure into a `neural_feat_pipeline.rs`.
Bindings: octree buffer + per-chunk base SSBO + the G-buffer images → feature buffer
(device-local). Shader = the ported `neural_feat_octree.comp`.

**Render-loop form, not harness form.** The ported pipeline does its own
submit+fence+host-IO (fine for a standalone smoke test). In the loop, drop the
submit/fence and **record `cmd_dispatch` + barriers into the frame command buffer** —
model on `view_compute_pipeline::record_dispatch` (already does in-frame dispatch).
Feat→shade buffers stay device-local and are wired pass→pass with a barrier between.

**Lean ship config (§11):** for the recommended config, also port
`neural_feat_lean.comp` (14 features, **0 octree gathers**, 6 hint rays only) from the
parallel engine — it matches the full feature set at a fraction of the cost.

---

## 4. The shade pass — and the fork that decides perf

There are **two** shade implementations, and shipping the wrong one throws the win away:

- **Reference (ported): `neural_shade.comp`** — the fused MLP with per-pixel scalar
  dequant. Correctness oracle only. BUGWATCH/perf: do **not** ship it (472 ms on
  lavapipe — naive structure).
- **Ship: the MLP as a tiled `linear_q8_0` GEMM** batched across pixels (3 linear
  layers + 2 silu). This is the kernel already bit-exact on your M2. It is **not yet
  ported** — `linear_q8_0.comp` + `silu.comp` live in the parallel engine and need
  bringing over (sandbox-compilable, §10), plus a small GEMM-chain pipeline.

Lean net topology: **14 → 64 → 48 → 3**. Keep `neural_shade.comp` as the parity
reference the GEMM path is diffed against.

---

## 5. Multi-chunk base table

The octree descent is validated single-chunk. For the full view: map world→chunk,
read `OctreeSlot::base_node_index()` (`octree_buffer.rs`), and pass `base` plus the
chunk-local coordinate `world & 31` into the feature shader. This is the only piece
of "new" logic; everything else is the validated single-chunk path.

---

## 6. Weight baking

Train the numpy `MLP` (parallel engine), export each layer via
`q8::quantize_layer(W, bias, out, d)` → 3×`(qw, qs, k, n)`, ship as an asset.
**Retrain on r300's continuous albedo** sampled from `out_galbedo` (§2, B2). Gate:
BUGWATCH F1 — quantize the same weights in the trainer and in `q8.rs`, assert
**byte-identical**, then assert GPU inference matches CPU within q8 tol (~6e-3). This
is the cheapest regression guard; re-run on any kernel/quantizer change.

---

## 7. Temporal (last)

Reuse `temporal_accumulate.rs` (already in r300 — the froxel reprojection +
neighbourhood-clamp) and the timewarp present pass. History = shaded colour + 6 hint
distances + worldpos + normal + age. Validate reuse by **position AND normal** (reject
ghosting), **force a full refresh past age==4** (kill drift), refresh a rotating 3/6
channels, raycast-fill disocclusions, TAA-blend. Result in the prototype: full-rate
quality at **2.4× fewer hint rays** (~2.5 rays/px/frame).

**BUGWATCH C1 (critical):** never store a view-dependent term in the history buffer.
Hint *distances* are view-independent surface properties (safe to reproject); any
view-dependent term (specular, Fresnel, a phase lobe) is applied **after**
reprojection with the current frame's view vector. Same rule that bit the froxel HG
phase.

---

## 8. Runtime validation checklist (M2-side — the sandbox can't run these)

Straight from the prototype's `INTEGRATION_BUGWATCH`, in the order they bite:

1. **Level-camera render → ground at the bottom** (A1: handedness / Vulkan Y-flip).
2. **One depth space** used for placement, depth test, and reprojection (A2); test
   off-center/edge pixels, not just a centered shot.
3. **`out_gworldpos` emitted directly**, not reconstructed from depth (B1).
4. **G-buffer channel encodings match the trainer**; retrain if albedo space changed (B2).
5. **No view-dependent term in any temporal/cached buffer** (C1).
6. **Volume depth-tested against the G-buffer**, verified on *near* geometry (D1/G1) —
   benign skybox shots hide the bug.
7. **Buffer index order + dispatch mapping asserted per-element vs the prototype** (E1).
8. **Q8 byte-identical trainer↔engine; GPU≈CPU within ~6e-3** (F1).
9. **Validate engine-vs-prototype with matched inputs, isolated variables, per-pixel
   diff** — not aggregate PSNR (G2); a localized misalignment hides under a flat metric.
10. **Per-pixel net only for surface integrals** (H1 — §9).
11. **Sparse-structure gathers continuous in query position** (I1 — §9).

---

## 9. Boundaries the prototype already proved

- **Surfaces only.** The per-pixel net gets +14 dB on AO/soft-shadow (local surface
  integrals) but makes *volumes worse* (god rays 20.8→19.3 dB with hint samples) — a
  per-pixel MLP can't represent spatial coherence (H1). So **fog stays froxel,
  reflections stay SSR** (roadmap), the net shades opaque surfaces.
- **Point-cloud features must be continuous** in the query position or they **facet**
  (I1: arbitrary-neighbour-subset + hard threshold + sparse cull → triangular
  artifacts, nearly invisible to PSNR). If a cloud feeds features: k-nearest
  (sorted) + Gaussian distance weights + soft angular falloff, and validate *visually*.
  (Note: the lean ship path drops the cloud entirely — 6 hint rays beat it on cost.)
- **Temporal only fights variance, not bias** (H2). It earns its complexity against
  genuinely stochastic sampling (the Monte-Carlo hint rays); it did ~nothing for the
  coarse-slice fog, whose error was integration bias (slice count).

---

## 10. Build order — sandbox-doable vs M2-side

**Done (r301):** forward-port — `q8.rs`, fused `neural_shade_pipeline.rs`, the two
shaders, compile + glslang gates green.

**Sandbox-doable next (compile-gated, can ship as releases):**
- Port `linear_q8_0.comp` + `silu.comp` from the parallel engine (the GEMM ship
  kernels, §4) — glslang gate.
- Port `neural_feat_lean.comp` (the 14-feature lean path, §3) — glslang gate.
- A `NeuralFeatPipeline` and a GEMM-chain shade pipeline (copy the existing
  pipeline's structure) — `cargo check` gate. (Compile-only; not runtime-valid until
  the framebuffer/dispatch wiring exists.)

**M2-side (needs live context + runtime validation):**
- `chunk.frag` MRT + the 3 framebuffer attachments + 4-attachment blend state (§2).
  *Pair the shader change with the attachment change* — a shader declaring outputs
  with no attachments fails validation at runtime.
- In-frame dispatch recording, pass→pass device-local buffers + barriers (§3).
- Multi-chunk base table (§5).
- Weight bake + albedo retrain + Q8 parity (§6).
- Temporal (§7).
- The §8 checklist + `bench_lean.py` (the perf gate).

---

## 11. Ship the lean config

`base + 6 hint rays (14 features), 64×48 net, no occupancy gathers`, MLP as the
`linear_q8_0` GEMM (not the fused shader, not 192×128). Prototype: lean **27.97 dB**
vs full(104,192×128) **27.33 dB** — *not worse* — at 7.4× fewer MACs and 0 octree
gathers. On the M2 the GEMM collapses toward its FLOP ceiling while raycasts stay
divergent, so cost ≈ the feature pass = **6 rays vs analytic's 22 ≈ 3–4× faster**,
and temporal amortizes the feature pass to ~2–3 rays/frame on top. Knobs if the MLP
is still heavy: shrink the net (96→64 trades a little of the +14 dB) or cut occupancy
features. `bench_lean.py` on the M2 gives the real number — that's the measurement
that decides it.

---

## 12. Determinism

All render-tier: per-player, f32, never hashed. The octree reads are the renderer's
existing reads; the G-buffer and history images are render resources. Nothing crosses
the sim boundary — no rollback concern (`sync_check` files prediction/reprojection in
the speculative tier). This is why neural shading is free to be non-deterministic
across GPUs: it never feeds the rolled-back deterministic core.

---

## 2a. G-buffer wiring against r300's `WorldRenderer` (applyable steps)

The fullgame drives `WorldRenderer` (offscreen HDR + postfx), and `chunk.frag`'s
forward pass already produces a correct image. The producer shader is in the tree:
`shaders/chunk_gbuffer.frag` (glslang-verified, reuses `chunk.vert`, emits the three
channels per the §2 contract). What's left is render-resource wiring, and it must be
**flag-gated so the default path is byte-identical to r300** — a wrong attachment
count is a hard runtime failure I can't catch without a GPU, and the whole visible
image is at stake, so this is built and validated on the M2.

**Recommended first form — a gated *parallel* G-buffer pass (zero regression):** do
NOT modify the existing offscreen pass. When `UNENDLICH_NEURAL_GBUFFER=1`, construct
and run a second pass that re-draws the chunk geometry into G-buffer attachments;
when unset, none of it is constructed and rendering is exactly r300. The redundant
geometry draw is acceptable for bring-up; once the G-buffer is validated, fold the
three outputs into the main offscreen pass (MRT, 4 attachments) to remove the
redundancy — that's the optimization, not the first step.

Pieces (all behind the flag):
1. **Images** — reuse `ctx.create_offscreen_target` (the `OffscreenTarget` helper):
   - `gworldpos`: `R32G32B32A32_SFLOAT` (B1 — full fp32, the sensitive channel).
   - `gnormal`, `galbedo`: `R16G16B16A16_SFLOAT` (fp16 is fine for these).
2. **Render pass** — 3 color attachments (loadOp CLEAR, storeOp STORE, final layout
   SHADER_READ_ONLY_OPTIMAL so the feature pass can sample) + a depth attachment.
   Reuse the offscreen depth image, or give the pass its own; either way use the
   SAME depth space as the forward pass (BUGWATCH A2).
3. **Framebuffer** — the 3 G-buffer views + the depth view, at the offscreen extent.
4. **Pipeline** — clone the chunk graphics pipeline but: fragment = `chunk_gbuffer.frag`,
   **same `chunk.vert` + same vertex input bindings/attributes + same camera-UBO
   descriptor set layout** (the vertex stage is unchanged), and a color-blend state
   with **3 attachments, blend disabled** (opaque G-buffer). Depth test on, same as
   the forward pipeline.
5. **Draw** — in `WorldRenderer`'s frame, when the flag is on, `cmd_begin_render_pass`
   the G-buffer pass and record the chunk draws with the SAME vertex/index buffers and
   the same camera descriptor set (just the gbuffer pipeline + framebuffer). The
   forward pass is untouched and still drives the visible image.

**Runtime validation (M2, the BUGWATCH order):** visualize each attachment first —
(A1) level the camera, ground must be at the bottom; (B1) `gworldpos` must vary
smoothly across a face (no depth-reconstruction banding); (A2) confirm the G-buffer
depth matches the forward depth on the same geometry. Then the per-pixel diff vs the
Python prototype on **near** geometry (G2/D1) — aggregate PSNR hides a localized
misalignment, and skybox/far shots hide a broken depth test.

**To write the exact pipeline clone**, I need `WorldRenderer`'s chunk pipeline
construction (the `VertexInputBindingDescription` / attribute descriptions + the
descriptor set layout it binds) and the spot where it records the chunk draws —
point me at those functions (or say the word and I'll read `world_renderer.rs` +
`postfx.rs` in full) and I'll write the gated pass + pipeline against them precisely,
rather than guessing the vertex layout.

---

## 6a. In-engine background training (the self-training loop)

Rather than train offline and ship weights, the net self-trains on the engine's own
features on a background thread. This sidesteps the offline step and makes the
trainer↔engine feature-encoding match (B2) automatic — the net learns on EXACTLY what
`neural_feat_gbuf.comp` emits.

Data flow:
```
 [ground-truth sample pass]                         [background thread]
  N random pixels/frame:                             BackgroundTrainer
   feature[14] (from G-buffer)   ── readback ──►  ring ─► Adam minibatch (TrainMlp)
   target[3]  = analytic shade                          │  every bake_every steps:
              × 22-ray AO/shadow (octree)               ▼
                                                   export_q8 → publish (generation++)
 [render loop]  try_take_weights() ◄──────────────────── (Arc<Mutex>)
        └─► hot-swap the shade pipeline's Q8 weight buffers
```

Built (r308, compile-verified):
- `render/neural_train.rs` — `TrainMlp`: a faithful Rust port of `mlp.py` (forward
  linear+silu / linear-out; backward MSE + **Adam**, β1=0.9/β2=0.999/ε=1e-8,
  bias-corrected — identical math, so it matches a numpy-trained net on the same
  samples). `export_q8` bakes each layer via `q8::quantize_layer` → the shade
  pipeline's `(qw, qs, k, n)`. `BackgroundTrainer` owns the thread + an mpsc sample
  channel + an `Arc<Mutex<baked weights>>` published behind a generation counter;
  `push(samples)`, `try_take_weights()`, `steps()`. `TrainMlp::self_test(steps)` trains
  on a synthetic mapping and returns the final MSE (sanity check the math learns).
- `WorldRenderer`: gated `neural_trainer: Option<BackgroundTrainer>` under
  `UNENDLICH_NEURAL_TRAIN`, spawned in `new`, stopped in `destroy`. Idles until fed.

Runtime / next (not built — octree-coupled, like the hint rays):
- **Ground-truth sample pass.** For N sampled pixels, cast the full 22-ray hemisphere
  AO + soft shadow through the octree (the prototype's expensive reference) and combine
  with analytic shade → `target[3]`; read the matching G-buffer feature → write
  `(feature, target)` to a host-visible buffer → read back → `trainer.push(...)`. This
  is the SAME octree-raycast-from-worldpos primitive the feature hint rays need, so
  building the octree march once unlocks BOTH the ground truth here and the hint rays
  in `neural_feat_gbuf` (§5) — do them together.
- **Hot-swap.** Each frame, `if let Some(b) = trainer.try_take_weights() { re-bake the
  shade pipeline's qw/qs buffers }` — the shade output then improves as training runs.
  (Requires the shade pass wired in-frame.)

Validation: `TrainMlp::self_test(2000)` should return an MSE well below the init (math
learns). On the M2, once the ground-truth feed is wired, `trainer.steps()` should climb
and the generation advance; the net converges over seconds. Knobs: `TrainCfg`
{ batch, lr, ring_cap, bake_every }.
