# Why llama.cpp's CPU path is fast, and where we could use it

Not a rewrite proposal. Three specific opportunities, in descending order of how well they fit what this
engine already struggles with.

## Where the speed comes from

1. **Hand-written SIMD dot products per quant type.** NEON on aarch64, AVX2/512 on x86. The hot loop
   consumes the PACKED format directly — it never dequantizes to f32 and then multiplies. Our GPU path
   does the same thing in spirit (`q4k_to_q4_resident`), so the idea is familiar; the difference is that
   llama.cpp has a tuned kernel per (quant type, ISA) pair.
2. **No dispatch overhead at all.** No submits, no command buffers, no driver round-trip, no watchdog. A
   session that has spent days on 1119 submits/token, staging fences and GPU timeouts should notice that
   an entire category of problem simply does not exist on that path.
3. **Bandwidth is the only wall, and on this hardware it is a high one.** M2 Ultra's unified memory gives
   the CPU real bandwidth. A 3B model at Q4 touches ~1.8 GiB per token, so even at a conservative
   100 GiB/s that is ~55 tok/s of headroom before compute matters.

## Opportunity 1 — the Liaison on CPU (best fit)

The Liaison is small, must be responsive, and must work WHILE the Thinker occupies the GPU. Running it on
the GPU makes it contend with the very model it is reporting on; running it on the CPU makes that
contention structurally impossible.

This is the case `modelroles::RolePlan::chat_ready` was written for — the Liaison being usable before and
during the Thinker's work — and a CPU Liaison satisfies it without any of the residency arithmetic. It
also sidesteps `has_model_headroom` entirely: CPU RAM is not the GPU buffer budget, so a 3B Liaison
alongside a 60 GiB Scout is free rather than a fit problem.

## Opportunity 2 — cold experts on CPU

Scout streams 12 cold experts per layer, and the fills are the origin of the staging ring, the completion
fence, the 200 GiB of traffic and most of the GPU timeouts. Those experts are already in host RAM.
Computing them there — instead of copying them to the GPU to compute and copying nothing back but a
vector — inverts the transfer: an expert FFN moves ~68 MiB of weights to produce a few KiB of output.

llama.cpp's `-ot ".ffn_.*_exps.=CPU"` is exactly this split, and it exists because the arithmetic favours
it whenever the expert is cold. The dense path and hot experts stay on the GPU.

## Opportunity 3 — the Disciple on CPU

Speculative decoding needs the Disciple to run AHEAD of the Thinker, not alongside it. On one GPU they
serialise and the speculation wins less; on separate processors they genuinely overlap.

## What this does not solve

Prefill of a large model is compute-bound and wants the GPU. So is any dense 30B+. The CPU path is for
small models and cold work, not a replacement.

## Honest unknowns

Nothing here is measured on the target hardware. The Liaison case is the cheapest to test — run a 3B
through llama.cpp on the M2 Ultra CPU while the GPU is busy with Scout, and see both the tok/s and
whether the GPU run slows. If the CPU model is fast and the GPU model is unaffected, opportunity 1 is
proven and opportunities 2 and 3 become worth the engineering.
