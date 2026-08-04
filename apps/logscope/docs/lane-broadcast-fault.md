# The Honeykrisp dynamic-shift fault

> **Superseded title.** This was filed as a "lane broadcast" fault. The workgroup-staged probe
> disproved that — it loads each word once from a distinct thread, barriers, reads workgroup memory,
> and still fails identically. The shared read was never the cause. See the correction at the end.


**Four threads reading one word return the value to one lane and zero to the other three.**

## The evidence

On an M2 Ultra under Asahi/Honeykrisp Vulkan, from `--gate-bytes`:

```
word_probe  dst[i] = src[i]        every thread a DISTINCT index   CORRECT, 64 of 64
byte_at     src[i >> 2]            four threads share ONE word     WRONG, 1 of every 4
```

Correct exactly where `i % 4 == 0` — the lane whose index *is* the shared one. The other three get
zero.

## What it is not

Each of these was tested and cleared before the shared-index pattern was isolated:

- **not the binding** — `bind[0] whole buffer, 64 bytes (16 u32)`, the whole range
- **not the upload** — `upload+readback` is byte-exact with no shader involved
- **not the element size** — `array<vec4<u32>>` fails identically to `array<u32>`
- **not the invocation index** — `id_probe` (`dst[i] = f32(i)`) is correct for all 64
- **not a small-buffer artifact** — the same 4-of-16 pattern at 64, 256, 4096 and 65536 bytes

Four wrong theories were proposed and killed along the way — std140 stride padding, a short binding,
a broken write path, a bad invocation index. Each was named before the cheaper test existed.

## Why it explains everything else

Every kernel that has been failing on this hardware reads one value from several threads:

- **Q4_K dequantization** — every thread in a 32-weight block reads the same `d`/`dmin` scale bytes.
  `gate-q4k` reported `gpu scale 1` — a default-looking value, which is what a lane that received zero
  and then normalized would produce.
- **Q6_K dequantization** — same shape, and `gate-q6k` showed 64 of 256 writes landing: **one lane in
  four**.
- **the subgroup matvec** — a subgroup reduction is shared-value by definition. It is disabled here for
  "hanging large matmuls", and wrong results are the more likely story.

One fault, three symptoms, treated as three bugs for days.

## The workaround

Make the broadcast explicit instead of relying on the compiler to scalarize a uniform load:

```wgsl
var<workgroup> staged: array<u32, N>;

if (local_id.x < N) { staged[local_id.x] = src[local_id.x]; }  // one distinct load per thread
workgroupBarrier();
let w = staged[shared_index];                                   // explicit broadcast
```

`BYTE_PROBE_STAGED` implements it, and `--gate-bytes` now reports whether it holds. Cost is a barrier
and a small threadgroup array per workgroup — cheap on a latency-bound decode, and the difference
between a correct kernel and a silent one here.

**Confirm it on hardware before restructuring anything.** If the staged form is CORRECT where `byte_at`
is WRONG, the fix for Q4_K, Q6_K and the subgroup matvec is the same mechanical change. If it also
fails, the fault is deeper than a scalarized load and the workaround is wrong.

## What this unblocks if it holds

- **Q6_K file views**, which stall the GLM-5.2 and Qwen3-30B residency paths
- **the subgroup matvec**, which is the kernel the in-tree analysis expects to win a latency-bound
  decode — the 182 ms/token against a 10 ms roofline
- and **Llama-4 Scout's gibberish** is no longer necessarily a Scout bug: every kernel indexing through
  a shared read is affected, on every model


---

# Correction: it is the DYNAMIC SHIFT, not the shared read

The workgroup-staged variant fails with the same 4-of-16 pattern. That form has **no shared storage
read at all** — one distinct load per thread into `var<workgroup>`, a barrier, then a workgroup-memory
read. If a shared read were the fault, staging would fix it. It does not.

So the shared-read theory joins the other four in the bin.

## What is actually common to every failing variant

```wgsl
word_probe   dst[i] = f32(src[i])                 no bit manipulation        CORRECT
byte_at      (w >> ((i & 3u) * 8u)) & 0xFFu       shift by a LANE-VARYING    WRONG
vec4         same expression, vec4 source         amount                     WRONG
staged       same expression, workgroup source                               WRONG
```

Lanes 0..3 shift by 0, 8, 16, 24 — and **correctness follows the lane whose shift is zero**. A shift by
a non-uniform amount yields zero on this driver.

## The corroboration that was already in the tree

`MATVEC_Q4` unpacks nibbles with `extractBits(word, (kk % 8u) * 4u, 4u)` and the comment beside it
reads **"AGX-safe dynamic offset"**. Someone hit this, worked around it, and recorded the workaround
without naming the fault. That is why the kernels that work, work — and why a "cleaner" rewrite to
shift-and-mask would silently break them.

## The test that settles it

`BYTE_PROBE_EXTRACT` keeps the shared read and the same byte, and changes **only** the extraction to
`extractBits`. `--gate-bytes` reports it.

* **`extractBits form CORRECT`** while the others are WRONG → the fault is the dynamic shift, and the
  rule is mechanical: *no shift by a lane-varying amount anywhere in a kernel*. That is a grep, not an
  investigation.
* **also WRONG** → the extraction is not it either, and what remains is the `& 0xFFu` mask or something
  about how the compiler lowers this whole expression class.

## Why this changes the outlook

A dynamic shift is a small, greppable pattern with a drop-in replacement that this codebase already
uses elsewhere. A lane-broadcast fault would have meant restructuring every kernel that reads a shared
scale. If the extractBits form is correct, the Q4_K and Q6_K dequantizers and the subgroup matvec need
an expression swap rather than a redesign.
