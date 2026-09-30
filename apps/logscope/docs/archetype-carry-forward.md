# Model Archetype: how context, state, and prompts relate per model kind

**Status:** implemented in `crates/gpu-gemm/src/archetype.rs`, wired into the engine's
carry-forward in `crates/logscope-egui/src/backend.rs`.

## The problem the archetype fixes

Every agent turn in the engine runs `generate(prompt)` on a *growing* prompt, and the engine
keeps a cache between turns so turn N+1 need not re-prefill the whole prompt from scratch. The
question the cache-reuse code has to answer each turn is:

> *Is the checkpoint carried from the previous turn valid to seed this turn's generation, and
> at what position?*

For a long time the engine answered this with a single token-equality test,
`prev_toks.iter().eq(prompt_ids[..prev_toks.len()].iter())`, and reported the outcome as one of
"KV prefix tree HIT / MISS". That conflated two *different invariants that live in the same
variable*, and that conflation is the whole defect this module exists to resolve.

### Two caches, one check, wrong answer for one of them

| Model kind | On-device cache | What the cache corresponds to | Valid reuse condition |
|---|---|---|---|
| Transformer | KV cache | **token positions** — the row at position `i` is only valid at position `i` | this prompt embeds the carried prompt as a *prefix of the token sequence* at identical token positions |
| Hybrid (Gated DeltaNet) | recurrent state `{ ring, s, next_pos }`, per layer | **the recurrent stream** — the state folds the stream over `0..next_pos` and cannot seek to an earlier position | this prompt embeds the carried *stream* at identical absolute positions, up to `next_pos` |

The old code folded both into "previous turn's tokens are a prefix of this turn's tokens". That is
the *correct* invariant for the Transformer KV cache. It is *also* the correct invariant for the
delta state **in the one hybrid the engine ships** — but only by accident: in the linear-agent loop
the recurrent stream *is* the whole prompt, so `token position == state position`, and a whole-token
prefix happens to coincide with a whole-stream prefix. The coincidence masked the fact that the two
invariants are distinct. The moment the driver stops feeding the delta state a clean full transcript
(injected context shifts the stream, a branch shortens it), the token-prefix check and the
stream-prefix check no longer agree, and the state would be seeded at the wrong position.

## The three axes

`Archetype` classifies a model; each axis is a separate question the planner asks.

### `Archetype` — what the model is

- `Transformer`: KV cache, no recurrent layers.
- `Hybrid`: attention blocks *plus* ≥1 Gated DeltaNet layer (what the engine ships; deltas ride
  alongside attention, never alone).
- `Recurrent`: a slot for a pure-recurrent checkpoint (none exists today).

Computed by `Model::archetype()` from `Model::deltanet_layers()`.

### `ContextSemantics` — what the KV cache corresponds to

- `AbsolutePrefix` (Transformer): rows keyed by absolute token position.
- `RecurrentStream` (Hybrid): the state is keyed by its own recurrent stream, not token positions.

This is the axis that used to be silently assumed to be `AbsolutePrefix` for every model.

### `StateSemantics` — what the on-device cache holds

- `KvCache` (Transformer).
- `DeltaNet { layers }` (pure recurrent — slot only).
- `Hybrid { deltanet_layers }` (attention + delta).

The numeric layer count is a model field (`Model::deltanet_layers()`); this enum names the *shape*
the caller must respect when seeding a fresh session.

### `PromptSemantics` — how prompts relate across turns

- `PrefixStable`: Prompt N = Prompt N-1 with tokens appended (the linear agent loop).
- `ReRendered`: each turn re-renders the prompt; only a shared prefix may be reused.
- `Windowed`: only a tail is kept.

## Position-absolute invariant, and why it is what it is

The recurrent state is **position-absolute**: `next_pos` is where the checkpoint sits, and a
recurrent layer can never seek to an earlier position. Two kernels enforce this with an assertion:

- `block_decode_deltanet_gpu` (lib.rs:16526): `next_pos.store(pos + 1)`, asserts `np == pos`.
- `block_prefill_deltanet_gpu` (lib.rs:16567): `next_pos.store(pos + s)`, asserts `np == pos`.

So the state folded over the previous turn's transcript sits at `next_pos == |transcript|`. Seeding
a fresh session at the wrong position would corrupt every downstream token; the assertion is the
protection. The carry is therefore valid **iff** this turn's prompt embeds the carried transcript as
a token-prefix at identical absolute positions (`prompt_ids[..generation] == carried_stream[..generation]`),
which is exactly the `RecurrentStream` invariant — the state was folded at position `generation`, and
it stays valid only where that position is preserved.

### Verified mechanics (this is what makes the carry position-correct)

- At snapshot time the carried transcript is `anchor_toks = prompt_ids + generated` and the state's
  `next_pos == s.pos == |prompt_ids| + |generated|`. Confirmed: `s.pos` is advanced once per token in
  `decode_step` (lib.rs:17193 in the batched path, and the per-layer delta routing at 17086-17106
  routes through `block_decode_deltanet_gpu`, which advances `next_pos` identically). So `s.pos ==
  delta next_pos` — they cannot drift.
- The carry seeds a fresh session at `next_pos == delta_l`, and `delta_l == prev_toks.len() == s.pos`
  at snapshot. It prefill-suffixes `prompt_ids[delta_l..]` and sets `s.pos = prompt_ids.len()`.
  Consistent: the state is carried at the position where the new prompt's suffix begins.

The old inline check `prompt_ids[..prev_toks.len()] == prev_toks` was already this exact invariant;
the archetype now *declares* it and routes both caches through it explicitly.

## The planning API

```
model.archetype().plan_context(previous_turn: Option<&ContextTurn>, current_turn: &ContextTurn)
    -> ContextPlan { prompt_position, kv_prefix: Option<usize>, recurrent_seed: Option<usize>, refused_reason: Option<Refusal> }
```

- `ContextTurn { prompt_ids, carried_stream, carried_generation }` — `current(prompt_ids)` for the
  current turn; `carried(prompt_ids, stream, generation)` for a turn carried from.
- `RecurrentPosition { stream_tokens, state_generation }` — the state's absolute position.
- `Refusal { NoCarriedTurn, PromptTooShort { prompt_len, needed }, StreamDiverges { at } }` — the
  planner's *why*, so the diagnostic reports the invariant that broke instead of a hand-computed
  `first_diff`.

`ContextPlan` sets exactly one of `kv_prefix` / `recurrent_seed` (both `None` when nothing may be
carried): for a Hybrid, `recurrent_seed` is the valid carry position and `kv_prefix` stays `None`
(the state is not a KV token prefix); for a Transformer, `kv_prefix` reports the reusable prefix
length and `recurrent_seed` stays `None` (no recurrent state to seed).

## Wiring (backend.rs)

- The carry snapshot stores the transcript, the carried `Session`, and the delta state's true
  `next_pos` as the tuple's third field: `prev_turn: Option<(Vec<u32>, Session, usize)>`.
- Each turn, the engine now builds the two `ContextTurn`s and calls
  `model.archetype().plan_context(...)`. `recurrent_seed` drives the delta carry (`delta_l`);
  `kv_prefix` feeds the Transformer KV prefix tree; `refused_reason` drives the carry-forward
  diagnostic (the inline `first_diff` comparison is replaced by the planner's own invariant check).
- Behavior is unchanged for the linear agent: `recurrent_seed == prev_toks.len() == s.pos` at
  snapshot, so the carried checkpoint is seeded at exactly the position the delta state was folded
  to.

## What this unblocks

The whole point of the carry-forward (the user's original ask) is that turn N+1 pays for the Δ
suffix instead of a full prefill. Until now the engine's carry could fire or refuse on the wrong
signal — the token-prefix check, which is fine for the KV tree, was also doing the delta-state work,
so a driver that injects context (shifting the recurrent stream) or shortens the transcript could
either corrupt the state or refuse a valid carry with an unhelpful log. The archetype makes the
carry decision a per-archetype, position-absolute question, so the two caches can no longer be
confused.

## Follow-ups (out of scope, noted for later)

- A `ReRendered`/`Windowed` driver would need the planner to accept a window/suffix, not a full
  transcript — the current `ContextTurn` carries the whole transcript.
- A pure-recurrent (`Recurrent`) checkpoint would go through `plan_hybrid` unchanged, since the
  invariant is identical — only the layer routing differs.
