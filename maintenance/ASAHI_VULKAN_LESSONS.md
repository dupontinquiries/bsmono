# ASAHI_VULKAN_LESSONS — what ramalama knows about the Asahi/Mesa/Vulkan stack, applied

*Reference: ramalama-main drop (2026-07-26), the container-native LLM runner. Mined for
Asahi-Linux/Mesa/Vulkan operational knowledge; applied to the logscope AI backends, unendlich GPU
compute, and the shared engine. Same ledger discipline as PERF_SHOCOVOX.md: every claim is marked
shipped-and-verified, shipped-pending-hardware, deferred, or not-applicable.*

## What ramalama actually encodes about Asahi

1. **Detection**: Asahi (Linux on Apple Silicon) is detected by reading
   `/proc/device-tree/compatible` and looking for the NUL-separated entry `apple,arm-platform`
   (`ramalama/common.py: check_asahi`). That's the whole check — no lspci, no Mesa probing.
2. **The GPU path is Vulkan, full stop**: the asahi accel preference list is `["vulkan"]` — no
   Metal (that's macOS), no GL, no vendor compute API. llama.cpp is built with plain
   `-DGGML_VULKAN=1`, identical to the generic Vulkan build. **All the Asahi-specific work is
   driver provenance, not build flags**: the asahi container installs `mesa-vulkan-drivers` from
   the asahi COPR (`asahi-repos`) — i.e. a Honeykrisp newer than stock Fedora's — plus
   `vulkan-loader` and `vulkan-tools` (so `vulkaninfo` is available to prove the stack works).
3. **Version pinning as policy**: llama.cpp is pinned to a commit (b9986-era, which contains the
   Jan-2026 GLM-4.7 sampling fix) and the generic Vulkan Mesa is pinned to an exact NVR
   (25.3.6-102.fc44). Known-good stacks are named, not floated.
4. **Never run on the wrong device silently**: the whole image-per-accelerator design exists so a
   CUDA box runs the CUDA image and an Asahi box runs the asahi image; device visibility flows
   through explicit env (`ASAHI_VISIBLE_DEVICES`, `GGML_VK_VISIBLE_DEVICES`, …).
5. **Serving levers for big models on small boxes**: `--ngl <n|auto|all>` (layers on GPU) and
   `--ncmoe N` (keep the MoE expert weights of the first N layers on the CPU) are first-class
   flags — the second one is exactly the knob a 30B-A3B like GLM-4.7-Flash wants on a
   unified-memory box when the resident set gets tight.

## Already in-tree before this pass (credit where it exists)

The fleet **is** Asahi M2 machines, and hard lessons were already recorded: logscope RULESET
**K2** (AGX/Honeykrisp miscompiles *dynamic plain shifts* in WGSL; the validated-safe idiom is
`extractBits` + non-underflowing `select`), the `conservative_vma` Apple clamp in gpu-gemm's
`Gpu::new` (BO-VMA ceiling below advertised limits), load-time **subgroup self-checks** that
disable the subgroup decode kernel with a printed reason, LANES.md's 2-submits-in-flight
compositor rule, and the Zink swizzle magenta workaround (`MESA_LOADER_DRIVER_OVERRIDE=asahi`) in
TODO.md. Nothing ramalama contains contradicts any of these.

## Shipped this pass

### 1. unendlich: K2 hazard class removed from the compute shaders — VERIFIED (sandbox), PENDING (silicon)
`chunk_generation.comp` packed/unpacked block bytes with **dynamic-amount plain shifts**
(`0xFF << shift`, `word >> shift`, `shift = byte_in_word * 8`), and `chunk_mesh.comp` had one
more read-side instance — the exact operation class K2 documents as miscompiled by the AGX
driver on real silicon. All three sites now use `bitfieldInsert`/`bitfieldExtract`
(SPIR-V `OpBitFieldInsert`/`OpBitFieldUExtract` — the same ops K2's WGSL idiom lowers to), with
the atomicAnd/atomicOr write structure unchanged.
**Verified here**: both shaders compile (glslangValidator, vulkan1.2); `harness/spv_reflect.py`
36/36; descriptor layouts byte-identical to `shader_descriptor_baseline.txt`.
**Honest caveat**: K2's miscompile was observed through the WGSL/naga path; whether
glslang-emitted dynamic shifts trip the same driver bug is *unproven*. This change is
hazard-class alignment with the only idiom validated on the actual hardware, not a claimed bug
fix. The existing on-device CPU cross-checks (`UNENDLICH_OCTREE_MESH_VERIFY=1` and the worldgen
mirror-of-`cpu_generator.rs` invariants) remain the proof when the kits run it.
Also audited: no subgroup ops anywhere in unendlich's `*.comp` (good — keep it that way, or gate
like gpu-gemm's load-time self-check if ever added); `s_halo[216]` = 864 B of workgroup memory,
~3% of AGX's 32 KB threadgroup budget.

### 2. gpu-gemm: no more silent software rasterizer — VERIFIED (harness)
`Gpu::new` requested a HighPerformance adapter and took whatever came back; on an Asahi kit with
a missing/shadowed Honeykrisp the Vulkan loader offers only llvmpipe, and decode would run ~100×
slow with nothing saying why (ramalama lesson 4). Now `adapter_is_software()` (llvmpipe,
SwiftShader, softpipe, "software", Microsoft Basic Render Driver) triggers a loud stderr
diagnostic naming the likely fix (asahi-repos mesa-vulkan-drivers, `vulkaninfo --summary`);
setting `LOGSCOPE_REQUIRE_HW_GPU=1` (for serve/fleet deployments) turns the warning into a
refusal. Default stays warn-only because CI runs llvmpipe deliberately. Classifier is pure with
tests covering the real reported strings and near-miss hardware names.

### 3. logscope backend: Asahi platform detection — VERIFIED (harness)
`llm_backend::is_asahi()` / `compatible_says_asahi(blob)` transcribe ramalama's device-tree
check (whole-entry match on the NUL-separated list, not substring). Wired for diagnostics and
for later backend labeling; pure parser is unit-tested against an M2-shaped blob, a
substring-trap blob, and non-Apple trees.

### 4. Verification method for the wgpu-29 crates — the harness
gpu-gemm and the logscope workspace **cannot build on this box at all**: wgpu-core 29 (via
eframe 0.34/egui-wgpu, and gpu-gemm's own `wgpu = "29"`) hard-requires an edition-2024
`indexmap`, and no `--precise` pin escapes it under cargo 1.75. Verification therefore ran on an
extracted harness: `moe.rs` + `archcfg.rs` **verbatim** (both std-only), plus
`adapter_is_software` + tests and the llm_backend pure fns (GLM-4.7 sampling presets, Asahi
parse) — `rustc --edition 2021 --test` → **29/29 green**. The harness caught two real bugs in
the GLM-4.7 changes before they could rot: `scale: f32` had broken `MoeConfig`'s `Eq` derive,
and four pre-existing archcfg tests still called the old 4-arg `moe_config`. Both fixed at
source. Regeneration: the python block lives in the session transcript; the shape is
"concatenate moe.rs, archcfg.rs, the classifier fn+tests, and the llm_backend pure blocks into
one `--test` file".

### 5. eframe/wgpu apps: request the ADAPTER's limits, never wgpu's defaults — SHIPPED (beatspace)
Reported from a kit: `beatspace gui` died before its first frame with
`WGPU error: Limit 'max_color_attachments' value 8 is better than allowed 5`.
Root cause is a request, not a capability: **eframe 0.35 renders through wgpu by
default** (its default feature list carries `wgpu`, not `glow` — verified in the
published 0.35 Cargo.toml), and its stock `device_descriptor` asks for
`wgpu::Limits::default()` on every non-GL backend. That default sets
`max_color_attachments: 8` (wgpu-types 27, verified), so any adapter allowing fewer
refuses device creation — even though egui draws into exactly ONE color attachment.
Fix in `apps/beatspace/src/gui.rs`: override `device_descriptor` to
`required_limits: adapter.limits()`, which is supported by definition — the same rule
gpu-gemm already states in `Gpu::new` ("Requesting the adapter's own limits is always
valid"). The override also prints the chosen adapter name/backend/driver, per lesson 4
(never-silent device selection): the report gave no way to tell WHICH GPU was picked,
and that one line would have answered it.
**Fleet note**: beatspace is the tree's only eframe-0.35 app (`tools/health/depdrift`
shows eframe at 0.28/0.29/0.34/0.35), which is why nothing else hit this. Anything
moving to 0.35+ inherits the wgpu-by-default renderer and needs the same override —
this is the pattern, not a one-off patch.

## Deferred / not applicable — with reasons

* **`--ncmoe` (n-cpu-moe) plumbing into GenParams**: the llama-cpp-2 Rust binding isn't vendored
  in-tree, so whether it exposes `n_cpu_moe` can't be checked here; guessing an API surface
  would be exactly the unverifiable-edit class this repo refuses. Recorded instead as the
  serving-level lever for GLM-4.7-Flash (llama-server `--ncmoe N` when the resident set
  approaches the 64 GB ceiling) — pick it up when the binding is present to check.
* **Mesa/llama.cpp version pinning policy**: adopted as documentation (this file + the GLM-4.7
  note that the Jan-2026 llama.cpp fix is the correctness floor for that model). The kits'
  install script should name an exact asahi-repos mesa NVR the way ramalama names 25.3.6; that
  script lives on-device, not in this tree.
* **`GGML_VK_VISIBLE_DEVICES` / `ASAHI_VISIBLE_DEVICES`**: honored by llama.cpp/ggml itself —
  nothing to implement; noted here so serving docs use the same convention instead of inventing
  a new env.
* **Image-per-accelerator containers**: not applicable — the fleet runs native, no podman layer.
* **16 K page size**: Asahi's 16 K pages are a known constraint class for mmap'd GGUF and
  jemalloc-style allocators; ramalama contains no specific handling (checked — no hits), so
  nothing to transcribe. Left as a watch item, not a claim.
