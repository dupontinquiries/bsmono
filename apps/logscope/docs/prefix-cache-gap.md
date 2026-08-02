# The KV prefix cache does not extend — every turn re-prefills the whole prompt

## The evidence

One chat, consecutive turns, Llama-3.2-3B on an M2 Ultra:

```
prompt  258 tok ( 258 new)   prefill 1.05 s
prompt  649 tok ( 391 new)   prefill 2.41 s
prompt 1117 tok ( 468 new)   prefill 4.49 s
prompt 1526 tok ( 409 new)   prefill 6.64 s
prompt 1784 tok ( 258 new)   prefill 8.44 s
```

**Prefill time tracks total prompt length, not the new suffix.** The last turn added *258 new tokens* —
the same count as the first turn — and took **8.44 s against the first turn's 1.05 s**. Same work, eight
times the cost, because it re-prefilled all 1784 tokens.

A cache that extends would prefill only what is new: ~400 tokens per turn, roughly constant, ~1.5 s.

## What is missing

`Gpu::attention_mh_ext` exists and is **never called**:

> Prefill-extend attention: `s` segment queries at global positions `[base, base+s)` attend over the
> cache already holding `base+s` rows, causal by global position.

That is precisely the operation a prefix hit needs — take the cache you already have, attend the new
segment over it, append. It was written, and nothing reaches it.

The loader reports `KV prefix tree ON — pool cap 3 (trades memory for fewer prefills on branchy/fan-out
chats)`, which is true for *branching* — a fan-out reusing a shared prefix avoids re-running it. What it
does not do is extend a cache along the same branch, which is the ordinary case of a conversation
getting longer.

## Why it went unnoticed

The line says the cache is on, and it is on. It saves prefills for the case it was built for. The linear
case looks like "long prompts are slow", which is true of every engine and therefore unremarkable — and
the per-token figure barely moves (4.07 → 4.73 ms/tok), so nothing in the log looks like it is degrading.

**The cost is invisible per turn and quadratic across a conversation.** Turn *n* re-does the work of
turns 1..n-1.

## The fix, and its size

Route a prefix hit through `attention_mh_ext` with `base` = the cached length and `s` = the new suffix,
rather than re-running the whole prompt. The function is written and documented; what is missing is the
call site and the bookkeeping to know how many rows the matched cache already holds.

This is the fourth instance of the class in `bug-classes.md`: **written, and unreachable.** It is also
the largest by effect — the q4 GEMM addresses prefill traffic within one turn, and this addresses
re-doing every previous turn.

## Order

Do this before the q4 GEMM. It is a call-site change against an existing tested function, where the GEMM
is a new kernel that cannot be validated off-hardware — and on a long chat this is worth more.


---

# Correction: the extend path works, and starts from zero

The section above said the extend machinery was unreachable. That is wrong and the correction narrows
the fix considerably.

`block_prefill_ext` — the per-layer extend — **is called**, from the memory-bounded chunked prefill
(`lib.rs:4852` and `10892`). Every prefill already runs as a sequence of extends: chunk 1 fills the
cache, chunk 2 extends it, and so on. The machinery is exercised on every single turn.

`attention_mh_ext` shows as unused because `block_prefill_ext` reaches attention by another route, not
because the extend path is dead.

**So what is missing is one decision, not a subsystem.** Chunked prefill always starts at `base = 0`
and walks the whole prompt. A prefix hit would start it at `base = cached_len` with the new suffix as
the remaining chunks — the same loop, entered later.

That makes this a considerably better first task than it looked:

* the extend path is not new code, and it is not untested — it runs on every prefill today;
* the failure mode of getting `base` wrong is loud (wrong positions, immediately visible output), not
  the silent degradation that has made everything else this session hard to verify;
* and it needs no GPU work at all.

What it does need is the bookkeeping to know how many rows a matched cache holds, and a lookup that
finds the longest cached prefix of the current prompt on the same branch — the linear analogue of the
fan-out `shared` computation in `backend.rs:1108`, which already does exactly this shape across
branches.

**The measured prize is unchanged**: 258 new tokens took 8.44 s on turn five against 1.05 s on turn one.
