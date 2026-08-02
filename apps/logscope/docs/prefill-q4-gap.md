# The prefill kernel gap: q8_0 has a GEMM, q4 does not

Measured on an M2 Ultra: Qwen3-30B-A3B, 1625-token prompt, **31.72s prefill** — about 51 tok/s.

## What the two paths actually are

```
m == 1        MATVEC_Q4          one row
m <= 8        MATVEC_Q4_SMALLB   reads each nibble once, accumulates all m rows
m >  8        LINEAR_Q4          strided — RE-READS THE WEIGHTS PER TILE
```

Prefill runs at `m = 1024`, so it takes the third. The in-tree comment on the small-batch kernel says
exactly what the third one costs: it exists to keep *"Q4's halved weight-bandwidth advantage instead of
the strided LINEAR_Q4 re-reading weights per tile"* — and then that advantage is given back above m=8.

Meanwhile `q8_0` has `linear_q8_0_gemm_rb`, a register-blocked GEMM measured at **2795 GFLOP/s**,
described in-tree as **2.8× the tiled kernel and 5.6× naive**, and it is the *default* — the tiled
variant is the opt-in escape, not the fast path.

**So a Q4_K model prefills on the kernel a Q8_0 model escaped from.** Q4_K_M is the format almost every
checkpoint ships in, which makes this the common case rather than an edge.

## Why it looks fine

Nothing is broken. The output is correct, the profile reports the GPU busy, and decode — which runs at
m=1 and uses `MATVEC_Q4` — is unaffected. The cost is confined to prefill, appears once per turn, and
is easy to attribute to "long prompt" rather than to the kernel.

It also does not show in the submit-cause breakdown, because the strided kernel is not doing *more
submits* — it is doing more *bandwidth per submit*.

## What the references do

llama.cpp does not have this split: its `mul_mat` dispatches by quantization type into the same
batched shape for every format, so Q4_K prefill and Q8_0 prefill differ in dequantization cost and not
in memory traffic pattern.

Colibri holds everything as int8-per-row and dequantizes on use inside one matvec — a single path, so
the question does not arise. That is the design consequence of converting to ONE canonical compute
format at load, which this engine deliberately does not do (it keeps q4 to halve the bytes).

Keeping q4 is the right call for decode, which is bandwidth-bound at m=1. The gap is that the same
decision was never revisited for prefill, which is compute-bound at m=1024 and has the opposite
tradeoff.

## The fix, and its size

A register-blocked q4 GEMM, mirroring `linear_q8_0_gemm_rb`: hold a tile of `a` in registers, read each
weight nibble once, accumulate across the tile. The q8_0 version already proves the shape works on this
hardware and compiles on every backend — the q4 version differs only in unpacking two weights per byte.

Gate it against the existing `MATVEC_Q4` reference the way `--gate-matvec` already does, and **run that
gate on hardware**: this codebase has twice shipped a kernel that was bit-exact on lavapipe and wrong on
an M2.

Expected: prefill moves from bandwidth-per-tile to bandwidth-once, so the ceiling becomes the same
compute bound q8_0 already reaches. Whether it lands near 2795 GFLOP/s is a measurement, not a promise.

---

# And decode is 6–10× off the same bound

Measured from real runs on an M2 Ultra and M2 Max:

```
Llama-3.2-3B Q8_0   3.20 GiB/tok → floor 12.4 ms, observed 76.0 =  6.1× off
DeepSeek-8B Q4_K    2.60 GiB/tok → floor 10.1 ms, observed 95.9 =  9.5× off
Llama-3.2-1B Q4_K   0.55 GiB/tok → floor  2.1 ms, observed 21.2 =  9.9× off
```

Consistent across sizes and quants, so it is structural rather than a bad shape.

## What has already been ruled out

**Kernel choice within the current set.** The autotune candidate list contains `V1` (one thread per
row, no reduction, no threadgroup memory), `V2`, `V2Wg128` and `V2WgSg`. It searches them per shape
and `V2Wg128` wins. So the occupancy argument — that a large `red[tile*128]` threadgroup buffer pins
concurrent workgroups on AGX, where each core has ~32 KB — was already put to the test by V1 and lost.

That matters: it means the 6–10× is **not** a tuning problem. Every non-subgroup kernel is that far off.

**Dispatch overhead.** 13–14 submits/token with 81% bind-group reuse, and setup time is 0.1–0.8% of
wall. The GPU is not waiting on the host.

## What remains

`V2WgSg` — the one kernel the in-tree analysis expects to win on this hardware, because it uses native
`subgroupAdd` with minimal or zero threadgroup memory. It is the ggml-metal shape: *"rows per SIMD
group"*, one 32-wide subgroup cooperatively reducing a column-tile with no threadgroup memory at all.

**It is disabled on three drivers because it is wrong on them.** So the decode speedup is gated behind
fixing the subgroup kernel, which is gated behind the short-binding finding from `--gate-bytes` — reads
past the first `u32` return zero.

**That is one chain, not three problems.** The same short binding plausibly explains the Q6_K gate, the
Q4_K gate, and the disabled decode kernel. Fixing it is the highest-value single action available, and
it needs a dispatch-time print of the bound buffer length on hardware to localise — the input branch
takes `as_entire_binding()` for `view: None`, which is correct on inspection, so the fault is either a
view arriving where none should, or the dims uniform cutting the loop short.


---

# The wide-load decode kernel exists and the autotune cannot reach it

`MATVEC_Q8_0_TILED_V3` — 128-bit `vec4<u32>` loads, four words per transaction instead of one. Its own
comment states the case exactly:

> The profile says decode is **latency/occupancy-bound** (≈6% BW, ≈1% ALU — the GPU idles on memory
> latency), and wider loads put more bytes in flight per request, the textbook fix.

That is precisely the 6–10×-off-roofline picture measured above: low bandwidth utilisation, near-zero
ALU, the GPU waiting on memory rather than working.

**But `MatvecKernel` is `V1 | V2 | V2Wg128 | V2WgSg`. There is no V3.** It reaches the GPU only through
`LOGSCOPE_MATVEC_V3=1` and a bench in `main.rs`, so the per-shape autotune — which searches every other
kernel and picks `V2Wg128` — has never compared against it.

That is why `V2Wg128` "wins": it wins the search it was entered in.

## What to try, in order

```
LOGSCOPE_MATVEC_V3=1        # the wide-load kernel, on a Q8_0 model
```

Q8_0 first because V3 is the q8_0 kernel — `Llama-3.2-3B-Instruct-Q8_0` at 76 ms/token against a
12.4 ms floor is the cleanest test available.

If it moves the number, the durable fix is adding `V3` to the `MatvecKernel` enum and the autotune
candidate list, so it competes per shape like the others rather than being an env flag someone has to
know about.

## Why this is independent of the subgroup chain

`V2WgSg` is blocked behind the short-binding finding and may take a while. V3 needs neither subgroups
nor any driver feature the guard has disabled — it is the same barrier-tree reduction with wider loads.
So it is the one decode speedup available *without* resolving `--gate-bytes`.

## What the references say

ggml-metal's decode kernels load `device const float4*` and `device const uchar4*` throughout — wide
loads are the default there, not an option. Colibri does the same on the CPU side with `float32x4_t`
accumulators, which is where the 2× integer-dot finding came from.

Both treat 4-wide as the baseline shape. This engine has it, benchmarks it, and does not select it.
