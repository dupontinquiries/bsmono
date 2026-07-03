# Speculative / Monte Carlo entropy decoding for AV1 — research notes

Filed for future revisit. Not on the critical path.

## The question

Range coding (used by AV1) is sequential by construction — symbol N+1 
depends on the entropy coder state after symbol N. This is the reason 
no AV1 software decoder ships a GPU path for entropy decode. The 
question: can Monte Carlo / speculative parallelism work around this?

## Short answer

Yes in theory, has been published, has not produced shippable wins.

## Body of work

Search terms: "parallel arithmetic decoding," "speculative entropy 
decoding," "Monte Carlo range decoding."

Notable references:
- Klein & Wiseman (2003) — early speculative arithmetic decoding work
- Patel et al. (2010s) — GPU-specific attempts on simpler entropy coders
- Various HEVC / VP9 / AV1 parallel decode papers exploring tile-level 
  rather than symbol-level parallelism

## Why it doesn't pay off in practice

Three structural problems:

1. **Verification serializes.** After N speculative streams decode, you 
   verify which trajectory was real by checking state continuity at 
   each boundary. That check walks the stream in order. The bottleneck 
   moves but doesn't disappear. The check is faster than full decode 
   (state matching, not symbol production) — gives constant-factor 
   wins, not asymptotic ones.

2. **State space combinatorial explosion.** AV1's range coder has 
   context-dependent CDF tables that update as decoding progresses. 
   The "state" isn't just (low, high) — it's also every context's 
   current CDF. Speculating across boundaries means speculating across 
   all possible CDF states. Bounded only by tile boundaries in AV1, 
   not at finer granularity.

3. **GPU memory bandwidth eats the speculation.** N speculative 
   decoders means N× working set. Modern GPUs are memory-bandwidth-
   constrained relative to compute. Speculative decode becomes BW-
   bound. Bandwidth scales with N. Published wins typically use N=4-16 
   on toy entropy coders, claim 2-3× — doesn't survive on full AV1.

## What ships

dav1d: CPU, SIMD. Tile-parallel multithreaded. No GPU entropy decode.
libaom: CPU. Reference. No GPU.
libgav1: CPU, ARM-focused. No GPU.
Hardware decoders (Intel/AMD/NVIDIA/Apple): fixed-function silicon 
  with multiple parallel entropy engines (one per tile), each still 
  sequential within its tile.

The people who would most benefit (browser teams, hardware vendors) 
have looked and chosen other approaches.

## Where it could pay off

**Resync-friendly bitstreams.** If you control the encoder, insert 
explicit resync points at regular intervals (forced byte-aligned bits, 
full state checkpoints). Speculative decoding between resync points 
becomes exact parallel decoding — no speculation waste, no verify cost. 
Some custom codecs do this. AV1 has it only at tile granularity 
(coarse). Adding byte-level resync would break the spec.

## The interesting modern angle

**Learned entropy decoding.** Small neural net predicts which 
speculative path is most likely correct. Prunes the speculation tree 
from "all states" to "top-K likely states." If K is small (4-8), GPU 
memory pressure goes away.

This is research, not engineering. Nobody has shipped it. Fits the 
"neural code observing AV1" framing from earlier conversations:
- Train predictor on actual AV1 streams
- Use predictor to guide speculation
- Fall back to sequential decode when prediction confidence is low
- Probably works. Probably a publishable result.

## File status

Notes only. Don't pursue until:
- Tuning GUI shipped and stable
- AV1 batch converter shipped and used
- Real measurements show entropy decode is actually a bottleneck for 
  some user workflow (it almost certainly isn't)

Picking this up later would be a 2-3 month research effort with real 
risk of zero shippable output. The learned-prediction angle is the 
most novel and most likely publishable.
