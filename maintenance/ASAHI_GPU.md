# Asahi / Honeykrisp GPU: what is measured, and what is only believed

One page for the whole tree, because two projects now run compute on this
hardware and were about to learn the same things twice. `logscope`'s
`gpu-gemm` has been running transformer inference on it for months;
`beatspace` is building a training path on it now.

Everything below is marked **MEASURED** (a run on real hardware produced it) or
**ASSUMED** (taken from documentation, or from one machine and generalised).
The distinction matters more here than usual — one of the constants in this
tree is a single measurement from one M2 Ultra applied to every Asahi machine,
and it says so in its own error message.

**Not a replacement for `ASAHI_VULKAN_LESSONS.md`, which sits beside it.** That
page is operational — how Asahi is detected (`/proc/device-tree/compatible`),
what the Mesa/Vulkan stack needs, how a container reaches the GPU. This page is
about writing a compute kernel once the device is open. Neither is complete
alone and they are cross-linked for that reason.

---

## MEASURED — the training GEMM works

```
adapter: Apple M2 Ultra (G14D B1) (Vulkan)
gemm 37x53x41 on a 3x3 tile grid
worst |gpu-cpu| = 1.669e-6 at (7, 24)
PASS — all 1517 elements within 1e-4
```

`beatspace gpu-selftest`, 2026-08-19, against the CPU reference in
`wgpu_train.rs` whose gradients are finite-difference checked.

**1.669e-6 is the right answer, not a near miss.** f32 has ~7 decimal digits,
the kernel accumulates 53 terms in tile order and the reference accumulates
them in index order, and a deviation at that scale is the reordering. A kernel
that was *exactly* equal would be suspicious — it would mean the tile loop was
not actually reordering anything.

The dimensions are deliberately ragged (37×53×41, none a multiple of the
16-wide tile), so this result covers the bounds handling in `a_at`/`b_at` and
not just the aligned interior.

## MEASURED — the parameters this GEMM uses

Validated by the run above, on `Apple M2 Ultra (G14D B1) [Vulkan]`:

| parameter | value | why |
|---|---|---|
| tile | 16×16 | two f32 tiles = 2 KB threadgroup memory |
| workgroup | 256 threads | eight whole 32-wide AGX SIMD groups |
| subgroup ops | **not used** | see below |
| barriers | two per tile iteration | one after load, one after the inner loop |

The second barrier is not optional. Without it a fast thread overwrites the
tile while a slow one is still reading the previous iteration out of it — a
race that produces plausible wrong numbers rather than a crash, which for a
training kernel is the worst available failure mode.

## MEASURED — the 65535 dispatch limit, the hard way

A 220,960-workgroup grid was accepted by the API and produced wrong results in
part of its output. It read as a training bug rather than a dispatch bug and
cost a day. Both projects now spill into `y`:

* `gpu-gemm`: `lib.rs` panics with the grid named, and `wg1()` spills.
* `beatspace`: `wgpu_run::Grid::for_items` spills by construction, so no
  caller can build the bad grid.

A kernel dispatched this way **must** derive its index from `num_workgroups`.
One that reads `gid.x` alone silently processes the first 65535 groups and
ignores the rest.

## ASSUMED — the AGX architecture numbers

From Apple-GPU compute documentation and ggml-metal practice, recorded in
`gpu-gemm/src/lib.rs`. Not measured by us:

* The SIMD group is 32 threads. Think in subgroups, not threads.
* Each core has ~32 KB of threadgroup memory, and a workgroup reserving the
  full amount pins occupancy to exactly one workgroup per core — which on a
  memory-latency-bound kernel is the dominant cost.

The practical rule both projects follow: keep any threadgroup reduction buffer
as small as possible, and prefer a shape that needs none.

## MEASURED — subgroup ops can hang this driver

`gpu-gemm` ships with subgroup matvec **disabled** on Honeykrisp:

> subgroup decode kernel DISABLED on Apple M2 Ultra (G14D B1) [Vulkan]
> (Asahi/Honeykrisp subgroup support can hang large matmuls;
> `LOGSCOPE_MATVEC_SG=1` re-enables)

This is the one place where the architecture argues one way and the hardware
argues the other. `subgroupAdd` *should* out-occupy a barrier tree here — no
threadgroup memory, native reduction — and it hangs. beatspace's GEMM
accumulates in registers and needs no reduction at all, which sidesteps it
rather than resolving it.

If someone re-tests this and it is fixed, the matvec path is where the win is,
not the tiled GEMM.

## ASSUMED, and loudly — the 31 GiB buffer ceiling

From `gpu-gemm`'s loader, quoted because the caveat is the important part:

> GPU budget 48.0 GiB clamped to 31.0 GiB — this driver accepts a large single
> allocation but will not hold more than that in buffers at once. **THIS FIGURE
> IS A COMPILED-IN CONSTANT, not a measurement of this device**: it was
> measured once on an M2 Ultra at 1 and 2 GiB slabs, mapped and unmapped, and
> every Asahi machine now receives that number.

The failure it prevents is a plan that dies *part way through* a load rather
than at planning time. `kernel-validate --gate-vram` measures the
single-allocation ceiling on the device in front of you; a load that fails
part-way is evidence the concurrent one is lower than 31.

`beatspace::wgpu_run::BufferBudget` refuses up front and repeats the caveat in
its error, so nobody reads the number as a property of their machine.

## MEASURED — pass boundaries dominate submits

From a real logscope run:

> 15.62 tok/s at 7 submits/tok — 96% of submits were forced by PASS BOUNDARIES
> with fusion NOT engaging (7200 passes for 9921 dispatches)

Consecutive dispatches can share a pass, and `LOGSCOPE_PASS_FUSE` is
**RETRACTED**: it broke AR decode 6/6 and changes generated tokens on 3 of 6
diffusion configurations. Do not reach for it. A training step has the same
shape — many small dispatches — and will hit the same wall; the answer there is
fewer, larger dispatches, which is the reason beatspace batches every
convolution's output positions into one GEMM rather than looping.

## The wgpu 27 API details that cost a compile each

* `device.poll(wgpu::PollType::wait_indefinitely())` — `PollType::Wait` is a
  struct variant and is not a value.
* `request_adapter` / `request_device` return `Result`, not `Option`.
* `ShaderModuleDescriptor::source` takes `Cow`, so `src.into()`.

---

## Where the code is

| what | where |
|---|---|
| inference kernels, autotuner, quant | `apps/logscope/crates/gpu-gemm/src/` |
| WGSL validation without a GPU (naga) | `gpu-gemm/src/wgsl_validate.rs` |
| training kernels + CPU reference | `apps/beatspace/src/wgpu_train.rs` |
| dispatch, budget, self-test | `apps/beatspace/src/wgpu_run.rs` |
| graphics-side perf method (p50/p99 seeded rows) | `games/unendlich/crates/vk-harness/perf/` |

`vk-harness` is graphics rather than compute, but its methodology is worth
copying when the training path starts producing timings: seeded rows tagged
`source:"hardware"`, tracked at p50/p99/max with a run count, so a regression
is visible against a recorded baseline instead of a remembered one.
