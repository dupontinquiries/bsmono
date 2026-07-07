# GPU HARNESS — two tiers for iterating on M2 GPU work

There are TWO tiers, mirroring how the pure-module work is gated. Use the sandbox tier to iterate fast;
the integration tier is the proof.

## Sandbox tier (runs HERE — no GPU, no cargo) — `harness/gpu_check.sh`
- Compiles every shader stage to SPIR-V with `glslangValidator` (#includes via `-I`) and validates with
  `spirv-val`. Catches won't-compile / invalid-SPIR-V — the bug class that otherwise only shows on-device.
- Plus the kernel's **CPU reference** module is gated by the normal `pure_test.sh` (rustc unit/property tests).
- This is what lets shader + kernel logic be iterated in the sandbox. Tools: `apt-get install glslang-tools
  spirv-tools`. (The chunk_cull workgroup-aggregation fix was validated exactly this way.)
- What it does NOT do: run the shader. No numeric behavior check here.

## Integration tier (needs cargo + Mesa lavapipe — run on M2 or a Vulkan-capable CI) — `crates/vk-harness`
- Runs the REAL shader on **lavapipe** (software Vulkan, `VK_ICD_FILENAMES=.../lvp_icd.json`) via
  `ComputeHarness` and **diffs its output against the CPU reference**. See `tests/mesh_parity.rs`
  (mesher shader vs `mesh_chunk_octree_cpu_reference`), `tests/compute.rs`, `tests/shaders_compile.rs`.
- This is the numeric proof: GPU == CPU reference, bit-for-bit (counts/bbox/values within eps).
- Run: `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test -p vk-harness -- --nocapture`.

## The contract for a new GPU compute kernel (e.g. the GPU_COMPUTE_PLAN field/genetics/pollination kernels)
1. **CPU reference first** — a pure module (R1-pure, `pure_test.sh`-tested) with the kernel's EXACT math.
   (R22 ships several: `field_step`, `popdyn_step`, `pollen_spread`, `plant_gene_express`.)
2. **Write the `.comp`** mirroring the reference; make it pass `harness/gpu_check.sh` (glslang + spirv-val).
3. **Layout parity** — push-constant + any UBO/SSBO structs match Rust↔GLSL (`ubo_parity.py`); reuse
   documented-free UBO slots (R3/R9/R16). Use the shared `ComputeHarness` binding/dispatch shape, not
   bespoke Vulkan plumbing (GPU_COMPUTE_PLAN #1 framework).
4. **Parity test** — add a `vk-harness` test that dispatches the shader on lavapipe and asserts equality
   vs the CPU reference (model it on `mesh_parity.rs`). Kernel is "done" only when this is green.
5. `.spv` regen on edit (R2).

## What I (sandbox) can iterate on vs what needs the device
- Iterate here: shader compiles, SPIR-V validity, the CPU reference's math + tests, push-constant/UBO
  layout, the diff/contract design.
- Needs the device (vk-harness/lavapipe, integration-tier): actual execution + numeric GPU↔CPU parity,
  performance. So I write the shader + CPU ref + parity-test scaffold here; the parity run happens where
  cargo+lavapipe exist.
