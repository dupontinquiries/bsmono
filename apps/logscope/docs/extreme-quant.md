# AQLM, QuIP# and the IQ family — what is worth supporting

## Short answer

Not AQLM or QuIP#. The accessible version of the same idea is llama.cpp's **IQ family**, and we read
`IQ1_S` (ggml type 19) but **not `IQ2_XXS` (16), `IQ2_XS` (17) or `IQ3_XXS` (18)** — which are the ones
people actually publish.

## Why not AQLM or QuIP#

Three reasons, in order of how decisive they are:

1. **You cannot convert to them locally.** Both require calibration against a dataset — GPU-days per
   model — to fit the codebooks. A GGUF cannot be transcoded into AQLM the way Q8_0 can be requantized
   to Q4_K. You are limited to whatever someone else has published, and that is a short list dominated by
   Llama-2-era checkpoints.
2. **The dequant is a gather, not a multiply.** A K-quant weight is `scale * quant`, which vectorizes.
   An AQLM weight is a sum of codebook entries looked up by index — a dependent load per group. On a
   decoder that is bandwidth-bound the trade is real but not free: you read fewer bytes and spend more
   per byte, and whether that wins depends on whether the codebook stays in cache.
3. **Neither is a GGUF format.** Supporting them means a second loader, a second tensor layout, and a
   second set of kernels, for models we mostly cannot obtain.

## What the IQ family would buy, on this machine

70B Llama-3.3, against the 48 GiB budget:

| format | bpw | size | fits |
|---|---|---|---|
| Q4_K_M (current) | 4.85 | 39.5 GiB | yes, barely |
| IQ3_XXS | 3.06 | 24.9 GiB | yes |
| IQ2_XS | 2.31 | 18.8 GiB | yes |
| IQ2_XXS | 2.06 | 16.8 GiB | yes |

The interesting column is not "fits" — Q4_K_M already fits. It is **what the freed space becomes**:

```
Q4_K_M   leaves  7.5 GiB of budget
IQ2_XS   leaves 29.2 GiB
```

At roughly 16 KiB per token of KV for a 70B, that is the difference between a context bounded by memory
and one bounded by patience. And it is the difference between a 70B fitting and a **120B-class model
fitting at all**, which is the case with no current answer.

## The cost that is usually understated

IQ2 quantization is **noticeably lossy**. Perplexity rises materially against Q4_K_M, and the loss is
not uniform — it concentrates in exactly the capabilities a coding assistant needs. A 70B at IQ2_XS is
not "a 70B with more context"; it is a different, weaker model that happens to have the same parameter
count.

That makes the honest framing: IQ2 is worth having as an OPTION for cases where context length or model
size is the binding constraint, not as a default. Which means it needs the same treatment as everything
else here — a gate that measures the loss on this hardware rather than a claim that it is acceptable.

## What would need to happen

1. `gguf::dequantize` learns types 16, 17 and 18. These are the fiddliest K-quants — codebook-based, with
   the codebooks compiled into llama.cpp as static tables that would have to be reproduced exactly.
2. `--gate-iq2` compares against a CPU reference, the way `--gate-q6k` did. Given that Q6_K took four
   faults to get right and three of them were invisible to inspection, the codebook tables are exactly
   the kind of thing to verify against a reference rather than review.
3. A perplexity or agreement measurement against Q4_K_M on a real prompt, so "noticeably lossy" becomes a
   number before anyone chooses it.

Not started. Recorded because the question "are we maximally using VRAM" has a real answer — we are not,
and the gap is IQ2/IQ3 rather than AQLM.
