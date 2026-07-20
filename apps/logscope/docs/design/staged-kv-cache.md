# Staged KV Cache — hot GPU window + demoted warm tail

Status: **design + pure core.** The GPU attention-fusion kernel is specified here and built incrementally
against the existing int8-KV path. Written against real seams in `gpu-gemm/src/lib.rs`.

## Why this, why now

The M2 Ultra run is **dispatch-bound at ~3 tok/s** — compute/bandwidth per token is the wall, not encoder
setup. KV bandwidth is a real chunk of that per-token cost, and it grows linearly with context. Two payoffs
from staging the KV:

1. **Speed**: attention reads less data per token when the long tail is int8 (4× smaller) instead of f32.
2. **1M context (the roadmap keystone)**: you cannot hold 1M tokens of f32 KV — at this model's
   `kv_dim=512`, 1M tokens × 2 (k,v) × 512 × 4B = **4 GB per layer**, ×48 = impossible. Demoting the cold
   tail to int8 quarters it, and it's the precondition for the DSA sparse-attention path later (DSA selects
   *which* warm blocks to read; staging is what makes them cheap to hold).

This is NOT the cross-session prefix pool (`KvPrefixCache`, already built — Hot/Warm *whole prefixes* across
conversations). This is staging *within one session's* KV: recent tokens hot, older tokens warm, in one
context.

## The model

A session's KV for each layer splits at a moving boundary `hot_start`:

```
  [ 0 .............. hot_start ) [ hot_start .......... pos )
  └─ WARM: int8, 4× smaller ───┘ └─ HOT: f32, full precision ┘
        (older context)              (recent window, size HOT_WINDOW)
```

- **Hot window**: the most recent `HOT_WINDOW` tokens, kept f32 (`KvCache`). Written every step exactly as
  today. Recent tokens dominate attention mass, so they stay full-precision.
- **Warm tail**: everything older, stored int8 (`QKvCache`). Demoted in blocks as the hot window slides.
- Attention at each step reads **both**: score against the warm int8 tail *and* the hot f32 window, softmax
  over the union, weighted-sum both value sets. This is the one new kernel-assembly piece; both halves reuse
  existing attention (`decode_attention_mh_q8` for warm, `attention_mh_full_t`/decode attention for hot).

## The seams that already exist (reuse, don't rebuild)

- `KvCache {k,v}` (f32) and `QKvCache {kq,ks,vq,vs}` (int8, 4× smaller) — the two tiers, both real.
- `demote_cache(&KvCache, len, kv_dim) -> QKvCache` (lib.rs ~5573) — f32→int8, already used by the prefix
  pool. Staging calls it on a *block* of the hot window as it slides out.
- `dequant_rows` / `promote_cache` — int8→f32 if a warm block is ever pulled back hot (rare; not needed for
  the basic policy).
- `decode_attention_mh_q8(q, kq, ks, vq, vs, len, …)` (lib.rs ~5591) — attention over an int8 KV span. The
  warm half of staged attention is exactly this over the warm tail length.
- The dense/f32 decode attention — the hot half.

So the new work is narrow: (a) a **staging policy** deciding when a hot block demotes (pure, testable, below),
(b) a `StagedKvCache` holding both tiers + the boundary, and (c) a **two-span attention** that fuses a warm
int8 score pass and a hot f32 score pass into one softmax. (c) is the only new GPU assembly.

## Staging policy (the pure core — built now, `kvstage.rs`)

All the decisions that don't need a GPU, so they're unit-tested in isolation:

- `HOT_WINDOW` (default 512): how many recent tokens stay f32. Tunable; larger = higher quality, more memory.
- `DEMOTE_BLOCK` (default 128): demote in blocks, not per-token, so demotion is amortized (one `demote_cache`
  call per 128 tokens, not per token) and the warm tail grows in aligned chunks (matches the int8 row layout).
- The policy is a pure function `plan(pos, hot_start) -> StageAction`:
  - `pos - hot_start <= HOT_WINDOW` → `Nothing` (hot window not full yet).
  - else → `Demote { from: hot_start, len: DEMOTE_BLOCK }` and advance `hot_start` by `DEMOTE_BLOCK`.
- Bit-exactness contract: with `HOT_WINDOW = ∞` (demotion disabled) staged decode MUST equal the current
  all-f32 decode, token-for-token. That's the first validation gate — the staging machinery must be a no-op
  when nothing demotes.

## Correctness plan (how we avoid silent garbage)

Staged attention is the dangerous kind of change — a wrong boundary or a mis-scaled int8 score doesn't crash,
it quietly degrades output. So the validation ladder is:

1. **Policy tests** (pure, now): the plan function demotes at the right positions, in aligned blocks, never
   loses or double-counts a token. `hot_start` monotonic; warm_len + hot_len == pos always.
2. **No-op gate** (GPU, next): `HOT_WINDOW=∞` staged == all-f32 decode, 0.000e0. Proves the plumbing.
3. **Two-span == one-span** (GPU): staged attention over [warm int8 tail | hot f32 window] matches a single
   int8 attention over the whole thing, within int8 tolerance — proves the fusion math (the softmax over the
   union is the same whether the split is there or not, modulo where the f32/int8 boundary falls).
4. **Quality delta** (on-device): perplexity/coherence of staged (HOT_WINDOW=512) vs all-f32 over a long
   prompt — the acceptance bar is "indistinguishable output," since recent tokens (which dominate) stay f32.

## Interaction with what's already there

- **Windowed decode**: staging is orthogonal — the window batches *sampling*; staging changes *attention's KV
  read*. They compose (staged attention inside each windowed slot). Build staging on the per-token path first,
  then thread it into `stream_session_window`.
- **The governor**: staging *reduces* buffer/memory pressure (warm tail is smaller), so it strictly helps the
  ceiling. The governor's budget doesn't change; staging just uses less of it.
- **int8 KV session (`SessionQ8`)**: already all-int8. Staging is the middle ground between all-f32 and
  all-int8 — best quality/speed tradeoff. `SessionQ8` stays as the max-context-min-quality option.

## Build order

1. `kvstage.rs` — pure staging policy + `StageAction`, fully unit-tested. **(this step)**
2. `StagedKvCache` struct (hot `KvCache` + warm `QKvCache` + `hot_start`) + the demote-on-slide wiring, with
   the `HOT_WINDOW=∞` no-op gate.
3. Two-span attention kernel assembly (warm int8 pass + hot f32 pass → fused softmax), gates 2 & 3 above.
4. Thread into the decode step; on-device quality check; then into windowed decode.
5. Only then: this is the substrate DSA sits on for 1M context (DSA picks which warm blocks to read).

## Open questions (flag, don't decide yet)

- Whether the warm tail itself gets *sub-tiered* for 1M (int8 → int4 for the very cold end). Deferred; int8 is
  the first cut.
- Whether `HOT_WINDOW` should be adaptive (grow when memory's free, shrink under governor pressure). Natural
  fit with the governor later, but start fixed + env-tunable.
