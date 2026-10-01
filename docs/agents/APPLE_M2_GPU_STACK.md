# The M2 GPU stack: Asahi, Mesa/Honeykrisp, Vulkan, `ash` and `wgpu`

Read this before touching anything that talks to the GPU on the author's
machine. It is the first document written from a session that ran ON the
hardware; older GPU notes in this repo were written without it (the perf-loop
ledger was empty until 2026-09-30). Every line is tagged:

* **[measured]** — observed in a session on this machine, command given.
* **[docs]** — taken from an older repo document, NOT re-verified here.

## The machine

| | |
|---|---|
| SoC / GPU | Apple M2 Max, `G14C B1`, integrated GPU, unified memory **[measured: `vulkaninfo --summary`]** |
| OS / kernel | Fedora Linux Asahi Remix 44, `7.1.13-402.asahi.fc44.aarch64+16k`, **16 KiB pages** **[measured: `/etc/os-release`, `uname -r`, `getconf PAGESIZE`]** |
| Vulkan driver | **Honeykrisp**, Mesa **26.1.8**, Vulkan API **1.4.354** **[measured]** |
| Other Vulkan device | `llvmpipe` (LLVM 22.1.8) is ALSO enumerated — a CPU device **[measured]** |
| ICD manifests | `/usr/share/vulkan/icd.d/{asahi,lvp,virtio,freedreno,…}_icd.aarch64.json` **[measured]** |
| Subgroups | `subgroupSize = 32`, `subgroupSizeControl = true`, `shaderFloat16`, `shaderInt8`, `storageBuffer8BitAccess` all true **[measured: `vulkaninfo`]** |
| Compute limits | shared memory 32768 B, 1024 invocations per workgroup, `maxMemoryAllocationCount` 4096 **[measured]** |
| Device heap | 31.32 GiB reported **[measured]** — the same number logscope carries as a hard-coded "GPU budget 31.3 GiB" constant, which its own log calls "a CONSTANT, not a measurement" and which `asahi-vram-ceiling.md` says has been wrong before **[docs]** |
| Timestamps | `timestampComputeAndGraphics = true`, period 1 ns **[measured]** |

**Two devices means selection matters.** unendlich's headless context scores an
integrated GPU (500) above a CPU device (10)
(`games/unendlich/crates/unendlich-engine/src/render/context.rs`,
`pick_physical_device_headless`), so Honeykrisp wins by default **[measured:
log line `picked device Apple M2 Max (G14C B1)`]**. To force the CPU device for
a comparison: `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.aarch64.json`
**[measured]**. `VK_ICD_FILENAMES` (older name) is what the older comments in
`context.rs` still say.

## The Rust stack, and which crate uses what

| layer | crate | version seen | used by |
|---|---|---|---|
| Raw Vulkan | `ash` (dlopens `libvulkan`) | 0.37 | unendlich engine (`headless` and `gpu` features), unendlich UI overlay |
| Portable GPU | `wgpu` (+ `naga`) | 29.x (`wgpu-types` 29.0.4) | logscope `gpu-gemm` and the app's wgpu backend |
| UI | `egui` / `eframe` | 0.34 | the fleet's `uikit-egui` apps (glow backend by default; see `eframe_backend_check.py`) |
| Camera (not GPU) | libcamera soft ISP via `gst-launch-1.0` | libcamera 0.7 | snapshot-cam, snapclone, tideshell (phone) |

Pick by need, not habit: `ash` when you need exact control of queues, memory and
SPIR-V (unendlich); `wgpu` when you want one WGSL source that also runs on the
phone's Turnip and on Metal (logscope). Do not mix versions of `ash` between
unendlich's engine and its UI overlay — they are deliberately pinned together.

## What the hardware does, as measured

**unendlich headless probe** (`gpu-headless-probe`, `--features headless`):
36 chunks, average **472–548 µs/chunk on Honeykrisp** (four runs: 548, 528, 505,
472) vs **1387–5351 µs on llvmpipe** (two runs; llvmpipe is very noisy). Both
runs reported GREEN. Context creation 42.9 ms. **[measured 2026-09-30]**
That is a 2.5–10× gap, not the orders of magnitude a GPU/CPU comparison
suggests, so this workload is mostly latency- and dispatch-bound, not
throughput-bound — do not assume more parallelism helps.

**logscope decode, Llama-3.2-3B Q4_K_M** (`perf-loop`, ollama stopped):
**12.94 tok/s at context 544, 16.0 at context 40**; ≈ 31 GB/s effective, about
8% of the 400 GB/s spec. One submit and ≈210 dispatches per token (the 210 is
printed identically in every run, including ablations, so it is probably a
constant, not a measurement). Fusion is a real lever: turning QKV fusion off
costs 25%, norm fusion off costs 31%. Decode tile, `FLUSH_EVERY` and the
bind-group cache did not move it. **[measured]** Full table:
`apps/logscope/perf-loop/LEDGER.md`.

**Driver faults that bite** **[measured / confirmed by `kernel-validate`]**:
* `array<vec4<u32>>` byte reads return WRONG values on Honeykrisp while scalar
  `array<u32>` is correct (`--gate-bytes`: "scalar OK | vec4 WRONG"). The fault is
  in the dequantizers, not the reader. Do not "optimise" a byte read into a vec4
  load without running that gate.
* Submit completion callbacks are slow or absent: the engine disables its
  submit governor ("submit stalled 250 ms") and bounds decode windows.
* The subgroup mat-vec is disabled by default here because it is documented to
  hang large projections **[docs: `LOCAL-AI-PROBING.md`]**. Use
  `kernel-validate --gate-sgladder`, one rung per run, never
  `LOGSCOPE_MATVEC_SG=1` at full scale.
* Host→device upload measured 5–12 GiB/s; the layer-streaming planner assumes
  100 GiB/s **[measured: `--gate-perf`]** — a ~10× error that matters for any
  model that does not fit resident.
* `bench.sh`'s roofline labels say "M2 Ultra / 745 GiB/s"; this is an M2 Max at
  ~400 GB/s, so its roofline percentages are scaled wrongly here.

## Hazards (read before running anything long)

1. A hung Honeykrisp dispatch can kill the GPU and need a reboot. A probe that
   hangs is data: write the note BEFORE the attempt (the ladder does).
2. `HK_SYSMEM` / `LOGSCOPE_MAX_GPU_GB` raise memory limits; raise in steps, not
   to the maximum, and never with something else large resident.
3. `bench.sh` refuses to run while `ollama serve` / `llama-server` is up. Note
   `pgrep -f` also matches a shell whose COMMAND TEXT contains those words —
   launch benches from a script file, not an inline command that names them.
4. Disk: this machine runs at 99% full. Delete `CARGO_TARGET_DIR`s when done.

## The phone is a different GPU

Fairphone 5: Adreno 643 on **Turnip** (Mesa), Vulkan available, GPU devfreq
315–812 MHz under `simple_ondemand` **[measured over SSH]**. It is aarch64 like
the laptop but postmarketOS/musl, not Fedora/glibc, so binaries do not carry
across without a musl build. See `apps/tideshell/deploy/` and
`apps/gitmind/DEPLOY_TO_A_PHONE.md`.

## Harness tiers for GPU work (unendlich)

1. **Sandbox tier** — `harness/gpu_check.sh`: glslang + spirv-val, no GPU.
2. **CPU-Vulkan tier** — the headless probe and `ComputeHarness` parity tests on
   lavapipe/llvmpipe. Correctness only; its timings mean nothing.
3. **Real-hardware tier (new)** — `harness/gpu_hw_check.sh`: the same probe on
   Honeykrisp, and it FAILS if the device it ran on is a CPU device, so a
   hardware run can never silently be a llvmpipe run. Fixed alongside it: the
   probe's summary line hard-coded "(lavapipe)" and so labelled real-GPU
   numbers as CPU ones; it now prints the driver actually used.

## Related, older documents (not re-verified here)

`apps/logscope/docs/asahi-headstart.md`, `asahi-vram-ceiling.md`,
`hardware-matrix.md`, `decode-vs-prefill.md`, `lane-broadcast-fault.md`,
`driver-quirk-hardening.md`, `apps/logscope/crates/gpu-gemm/LOCAL-AI-PROBING.md`.
