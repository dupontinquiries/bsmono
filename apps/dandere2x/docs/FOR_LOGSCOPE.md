# What dandere2x measured that may help logscope

dandere2x and `apps/logscope/crates/gpu-gemm` push the same hardware through
wgpu compute with different shapes: gpu-gemm runs ~210 small dispatches per
token and is launch-bound; dandere2x runs ~398 large dispatches per tile and
is ALU-bound inside the kernel. Because the two sit on opposite sides of the
same roofline, each one's measurements answer questions the other cannot ask
cheaply. This is the dandere2x half, written for whoever picks up gpu-gemm.

Everything below is measured on the same box as
`reference-docs/ENGINE_PERF_STATUS.md` — Apple M2 Ultra (G14D B1), Honeykrisp,
Mesa 25.3.6, Fedora asahi 7.1.6 — using `dandere2x bench`, which times full
tiles and reports GMAC/s. Every kernel named is bit-exact against a CPU
reference before it is timed, so none of these are speed-vs-correctness
trades. Where a number is from lavapipe it says so.

## 1. The headline: registers are the currency on AGX, not multiplies

Same convolution, same data, eight kernels differing only in how work is
blocked per thread (RealESRGAN x2, tile 688, f32 peak ≈ 13.6 TFLOPS):

| kernel | GMAC/s | shape per thread |
|---|---|---|
| rt1 | 661 | 1 px × 4 ch |
| wino | 677 | Winograd F(2,3), 2.25× fewer multiplies |
| rt4 | 683 | 4×4 px × 4 ch (~130 live registers) |
| rt2 | 896 | 2×2 px × 4 ch |
| rt2c8 | 977 | 2×2 px × 8 ch |
| **rt2f** | **1022** | rt2 + interior fast path |
| rt2c8f | 803 | rt2c8 + fast path (duplicated branch) |
| rt2c16f | 495 | 2×2 px × 16 ch + fast path |

Three transferable findings:

**Winograd is a trap here.** F(2×2,3×3) cuts multiplies by 2.25× and *lost*
(677 vs 896). It needs 16 accumulators, and on AGX the occupancy cliff past
roughly 64 registers costs more than the arithmetic saves. Anyone tempted to
put a fast-transform GEMM into gpu-gemm should price the register footprint
first — the multiply count is not the thing being bought.

**Register-blocking has an interior optimum, and it is small.** The curve is
an inverted U peaking at 2×2 pixels. Going up (rt4) and down (rt1) both lose
~25%. The usual "more work per thread = better arithmetic intensity"
intuition inverts on this hardware once the working set stops fitting.

**Grow the *channel* axis, not the pixel axis.** rt2c8 reuses one loaded
input patch across two weight groups: it doubles arithmetic per load while
the input-register footprint stays identical to rt2. That is the one
direction that pays (+9%). For gpu-gemm the analogue is blocking more output
columns per thread rather than more rows of A.

## 2. Duplicated branch bodies spill; hoist instead

rt2f wins by giving threads whose input patch is fully interior a path with
no per-scalar bounds test. But composing it with channel blocking *lost
badly* (rt2c8f 803, rt2c16f 495) because the fast path was written as a
duplicated branch: with two channel groups the shader carried four copies of
the accumulator block, with four groups eight copies, and the allocator
spilled.

The structural fix, and the general lesson: address arithmetic that depends
only on the thread's position does not belong inside the reduction loop.
dandere2x's generic load did four comparisons, two selects and a divide *per
scalar*, recomputed on every one of up to 256 input channels. Hoisting it
into per-thread row bases, column offsets and 0/1 masks gives the same effect
with a single copy of the inner block (`conv3x3_rt2h.wgsl`,
`conv3x3_rt2c8h.wgsl`). If any gpu-gemm kernel recomputes index math or
bounds tests inside its `k` loop, that is free throughput.

## 3. On unified memory, in-flight overlap is a net loss

dandere2x added rotating activation sets so tile k+1's upload and tile k−1's
readback overlap tile k's compute, waiting per submission index rather than
on the whole queue. On the M2 Ultra it does nothing — 897 GMAC/s at 1, 2 and
4 sets — and it actively *hurts* when memory is tight, because each extra set
steals VRAM from tile size: a 2 GB budget at 4 sets shrinks the tile to 240
and costs 27% (637 vs 878). There is no PCIe transfer to hide on UMA. The
default is now one set.

This is worth knowing before building an N×K locked pool (ENGINE_PERF_STATUS
§5.3) for overlap reasons — on this hardware the pool's value is dispatch
amortisation, not transfer hiding, and those are different arguments that
happen to point the same way.

## 4. Throughput is flat across tile size — read that as a per-thread ceiling

896 ± 2 GMAC/s at tiles 400, 512, 640 and 688; 1015 for a completely
different network (SRVGG, 30× cheaper per pixel) at tile 1024. When the
number does not move with problem size *or* with network shape, the limit is
per-thread issue, not occupancy tail, launch overhead or DRAM. That is the
signature that sent us to kernel blocking; the mirror-image signature in
gpu-gemm (throughput that *does* move with batch/window size) is what makes
fusion the right lever there instead.

## 5. Two gpu-gemm lessons that apply straight back to dandere2x

Reading ENGINE_PERF_STATUS caught two mistakes in the dandere2x runner that
are still open, recorded here so the debt is visible:

- **dims-uniform cache (`640c5fa`).** dandere2x creates its ~398 params
  uniforms once, but rewrites all of them on *every tile* — and with balanced
  tiling every tile has identical dims. Same mistake, one level up.
- **submit granularity.** One submit and one wait per tile, where the whole
  frame's tiles could go in one encoder ("K steps in one submit").

Neither is fixed yet. At ~2.5 s per tile they are small here, but they will
matter for the compact model, where a tile is ~665 ms and 398 dispatches of
per-dispatch overhead is a larger fraction.

## 6. Small facts that cost time to learn

- **`unpack2x16float` is not free core WGSL under naga 29** — it is gated
  behind `SHADER_FLOAT16_IN_FLOAT32`, so an f16-weight path needs a device
  feature and a probe, not just a shader edit. This matters for gpu-gemm's
  quantised paths: reading f16 weights in an f32 kernel is a capability
  request, not a formatting choice.
- **wgpu 29 churn** (both crates will hit this): `InstanceDescriptor::default()`
  is gone in favour of `new_without_display_handle_from_env()` passed by
  value; `enumerate_adapters` is async; `PipelineLayoutDescriptor` takes
  `&[Option<&layout>]` and `immediate_size` instead of push-constant ranges;
  `device.poll(PollType::Wait { submission_index, timeout })` waits on one
  submission rather than the queue.
- **A dense block that reads `[0, n)` and writes `[n, n+32)` of one buffer**
  needs every activation binding declared `read_write` — wgpu rejects mixing
  read-only and read-write views of the same buffer in one bind group, even
  when the ranges are disjoint.
- **Bit-exact selftests make speed work cheap.** Every kernel variant is
  checked against a CPU reference (±1 LSB for f32 reordering) before it is
  timed, which is why eight variants could be tried in two days without ever
  wondering whether a fast one was also a wrong one. `dandere2x selftest
  --kernel X` is ~2 s per variant.

## 7. What is still unknown

Whether hoisted addressing (`rt2h`, `rt2c8h`) recovers rt2f's win *and*
composes with channel blocking — it is generated and bit-exact but has not
been timed on hardware. And whether f16 weights help once the capability is
requested: weights are 82% of this kernel shape's load bytes, which is the
largest untouched lever in either codebase.
