# The Honeykrisp lane-broadcast fault

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
