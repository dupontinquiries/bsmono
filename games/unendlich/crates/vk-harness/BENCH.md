# lavapipe relative-performance bench

`tests/bench.rs` records GPU-side kernel time (Vulkan timestamp queries, `timestampPeriod = 1 ns`)
for compute kernels on **lavapipe** (Mesa's software Vulkan device), in the same
`min / p50 / p90 / p99 / max` shape as the engine's hardware GPU-timer summaries.

## What this is — and is NOT

lavapipe is a **CPU** implementation. Absolute numbers here have **no relationship** to the
real M2 / Honeykrisp GPU and must never be quoted as hardware performance.

What it *is* good for: a **relative proxy**. The same kernel, on the same machine, before vs
after a change. It reliably catches:

- algorithmic complexity changes (a loop that went O(n) → O(n²), an extra full-buffer pass),
- large swings in work per invocation,
- accidental dispatch-size blowups.

It does **not** model real-GPU cache behaviour, occupancy, memory bandwidth, or wavefront
divergence — and on this shared sandbox CPU, cross-run variance is large (see below), so
only big deltas are signal.

## How to track before vs after

```
# baseline (before a change)
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
VK_HARNESS_BENCH_LOG=/tmp/bench_before.jsonl \
  cargo test -p vk-harness --test bench -- --nocapture

# … make the change …

VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
VK_HARNESS_BENCH_LOG=/tmp/bench_after.jsonl \
  cargo test -p vk-harness --test bench -- --nocapture

diff <(jq . /tmp/bench_before.jsonl) <(jq . /tmp/bench_after.jsonl)
```

`min_ns` is the most stable statistic (least scheduler noise); compare that first.

### Cross-run variance is large here — trust only big deltas

Observed in practice: the `octree_sample_32cubed` min jumped from **413 µs to 728 µs**
between two separate `cargo test` invocations on this shared sandbox CPU — ~75%, with no
code change. lavapipe's "GPU" timestamp is really CPU time on a software rasterizer, so it
tracks OS scheduling, CPU-frequency, and contention from concurrent builds in the sandbox.
Normalizing to the `double` baseline doesn't fully cancel it (the kernels scale differently
under load).

Consequences for honest use:

- A delta between two *separate invocations* is only trustworthy if it's **large** (≳2×) or
  reproduces across several repeated runs.
- For sensitive before/after, bench the **old and new kernel in the same process run** (add
  both to one `#[test]` and compare them against each other in that run) — within one
  invocation the machine state is shared, so the relative ordering is far more stable than
  absolute numbers across invocations.
- The baseline table below is a single illustrative snapshot, not a fixed reference; expect
  the absolute numbers to drift run-to-run.

## Baseline snapshot (lavapipe `llvmpipe (LLVM 20.1.2, 256 bits)`, 64 iters, microseconds — illustrative)

| kernel | min | p50 | p90 | p99 | max |
|---|---|---|---|---|---|
| `double_u32x4096` (baseline sanity kernel) | ~10 | ~10 | ~14 | ~15 | ~17 |
| `octree_sample_32cubed` (real `octree_sample.glsl`, 32³ cells) | 410–730 | ~440–760 | — | — | — |
| `mesher_octree_8cubed` (real `chunk_mesh_octree.comp`, full 32³ dispatch, 8³ solid emit) | ~5900 | ~6000 | ~6100 | ~6600 | ~7800 |

The mesher bench runs the engine's real `chunk_mesh_octree.comp` over all 32,768 cells (the
dominant, scene-independent traversal cost) with a solid 8³ shell to exercise the emit path.
Capacities are sized so the atomic counters stay under capacity across all iterations
(`bench_compute` re-submits one command buffer, so counters accumulate) — otherwise the emit
path would early-out on overflow and distort the timing. A change to traversal, face culling,
neighbor sampling, or emit that regresses the proxy by a large factor shows up in `cargo test`.

## Finding: buried-cell skip is ~flat (the emit path dominates, not traversal)

`bench_mesher_buried_skip_ab` A/B (lavapipe, same-process, set `OLD_MESHER` to a
pre-optimization `chunk_mesh_octree.comp`):

| shader | solid_32³ p50 | min |
|---|---|---|
| OLD (no skip) | ~19608 µs | ~19545 |
| NEW (buried-skip) | ~19302 µs | ~19178 |

≈ **1.02×** — within noise. The reason is structural and worth remembering: a
fully-uniform chunk collapses to a shallow octree (root is a single leaf), so the
six neighbor "descents" the skip avoids are each just one node read. The real cost
of a dense dispatch is the **emit path** — ~24.6K vertices written via atomic
counters — which the skip doesn't touch. And the cases with the most buried cells
are exactly the ones with the shallowest octrees, so traversal is never the
bottleneck there. The skip is kept (correct + free, may differ on real-GPU
divergence) but the lever for a real win is the emit path: greedy meshing at Near
LOD would cut a solid face from 1024 quads to a handful, shrinking emit + upload +
draw. Confirm any of this on the M2; lavapipe is a CPU device.
