# The subgroup matvec is disabled by a NAME CHECK — the fixture theory was wrong

> **Correction, from the first working Scout run.** I theorised that a corrupt fixture had disabled the
> subgroup kernel through `self_check_sg`. It cannot have. `sg_ok` is forced false by a preflight
> **adapter-name check** — `if (asahi_vulkan || cfg_off) && !force_sg` — *before* `self_check_sg` runs.
> The self-check's verdict on this hardware has never mattered.
>
> I wrote a commit about a mechanism without reading what actually sets the flag. The rest of this
> document still holds for the two probes that are genuinely fixture-dependent.


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


---

# Three probes shared the poisoned packer, not one

Sweeping for everything that packs weights and then judges a result found two more:

| probe | decides | failure mode |
|---|---|---|
| `self_check_sg` | whether the subgroup matvec is usable | **loud** — prints "subgroup decode kernel DISABLED" |
| `probe_valid_gemm_tiles` | which GEMM tiles enter the autotune | **silent** — a smaller candidate set still works, just slower |
| `probe_expert_compute` | the MoE expert compute path | quiet |

All three pack through `QUANT_PACK` or `Q4K_TO_Q4`, both of which wrote bytes with a lane-varying
shift until `_526`.

**The GEMM tile one is the worst of the three**, because it fails without saying so. A tile that
produced a "wrong" answer over corrupt weights was dropped from the candidate set, the autotune chose
from what remained, and the log reports a tile as though the search had been complete. That is the
same structure as `V3` being absent from the decode candidate list — *an autotune result reads as
evidence, and it is only evidence about its candidate list.*

It also means the prefill measurement is suspect from the other end. Prefill runs the register-blocked
GEMM at a tile the autotune picked, and on this driver that pick came from a set that may have been
pruned by corrupt fixtures. The **4% of device bandwidth** figure in `prefill-q4-gap.md` was measured
under that condition.

## What one load now re-decides

All three probes run at load and all three are now fed correctly packed weights. So the next run
re-answers, for free:

* whether the subgroup kernel is usable (loud, in the preflight line);
* which GEMM tiles are valid (silent — visible only as a different tile in `per-shape autotune`);
* and what the expert compute probe concludes.

None of that required new code. It required the fixtures to stop lying, which took finding one
misbehaving shift.


---

# What the working run actually says about speed

Scout, `slab=1024 GB=28`, correct output:

```
produced 661 tokens in 529.0s = 1.25 tok/s
phase balance — decode is 7.3x SLOWER PER TOKEN than this model's own prefill
(an earlier turn in the same run: 76.2x)
bind-group cache — 137895 hits / 6890 misses
```

**The 7.3× is the number to chase, and it is now measured on a build that produces correct text** —
every previous decode figure was taken on kernels that mis-unpacked three bytes in four.

**The bind-group cache is doing its job**: 95% hit rate, so dispatch setup is not the cost.

**The subgroup kernel is still off, and by a name check rather than a measurement.** Its stated reason
— "can hang large matmuls" — predates the discovery that this driver mis-executes shifts by a
lane-varying amount, which made every kernel comparison on it unreliable. `LOGSCOPE_MATVEC_SG=1` is
now worth trying on a build where the packers are correct.

**The per-shape autotune still reported loading 41 decode + 14 GEMM tiles from cache**, despite the
epoch bump in `_529`. Either that line reads a different file from `autotune_file()`, or the bump did
not reach the path that logs it. Worth checking before trusting any tile the log reports.

## Order for the next speed run

1. `LOGSCOPE_MATVEC_SG=1` — the guard is unmeasured on a correct build, and the subgroup kernel is the
   one the in-tree analysis expects to win a latency-bound decode.
2. Check whether the per-shape cache is genuinely cold; if not, find the second cache file.
3. `LOGSCOPE_MATVEC_V3=1` — vec4 loads, in the autotune since `_415`, still never measured on hardware.
