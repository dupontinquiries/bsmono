# The subgroup matvec may have been disabled by a broken fixture

## The chain

`V2WgSg` — native `subgroupAdd`, minimal threadgroup memory, the ggml-metal "rows per SIMD group"
shape — is the kernel the in-tree analysis expects to win a latency-bound decode. Decode measures
**182 ms/token against a 10 ms roofline**, and prefill moves the same weights **14× faster per token**,
so the gap is latency the m=1 path has nothing in flight to cover. That is exactly what a subgroup
reduction with no threadgroup buffer addresses.

It is disabled here. `self_check_sg` runs at load, compares `matvec_decode_v1` against
`matvec_decode_v2sg` on the same weights, and sets `sg_ok = false` when they disagree.

**That check packs its weights with `quantize_q8_resident`, which uses `QUANT_PACK`** — a shader that
until `_526` wrote its bytes with `packed | (byte << (b*8u))`. A shift by a lane-varying amount, which
`--gate-bytes` proved returns zero on Honeykrisp for every lane whose shift is non-zero.

So on that driver the check has been comparing two kernels over **corrupt weights**. And the two do not
unpack identically — V1 reads through a `byte_at` helper that carried the same fault, while the
subgroup kernel contains **no lane-varying shift at all**. Different corruption on each side, a
disagreement, and `sg_ok` false.

**The kernel may never have been wrong. The fixture was.**

## Why this was not obvious

The failure was attributed to "Asahi/Honeykrisp subgroup support can hang large matmuls", which is
plausible, has precedent on that driver, and was never re-examined. A disabled feature with a
reasonable explanation attached is one nobody re-tests.

## What tests it, for free

Nothing new is needed. `self_check_sg` runs on every load, and `_526` fixed both `QUANT_PACK` and the
five `byte_at` helpers. So on the next load either:

* the preflight line **stops** saying `subgroup decode kernel DISABLED`, and the kernel was fine all
  along — in which case the autotune will consider `V2WgSg` per shape and the decode number becomes
  interesting; or
* it still says DISABLED, and the subgroup path has a real fault independent of the shift — which is
  now a much narrower claim, because the fixture that produced it is no longer suspect.

## If it unlocks

`V3` (128-bit `vec4` loads, wired into the autotune in `_415`) attacks the same latency from the other
side and has also never been measured on hardware. Both would compete per shape on the next load, and
the `decode tile = N (kernel ...)` line names the winner.

That would make the first honest measurement of the 6–10× decode gap possible — every previous number
was taken with the two candidate kernels excluded, one by a broken fixture and one by never being in
the candidate list.
