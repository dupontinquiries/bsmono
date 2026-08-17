#!/usr/bin/env python3
"""diffuse_ref.py — a reference BLOCK-DIFFUSION LOOP over llada2_ref's verified forward, used to
check gpu-gemm's `diffuse_generate` end to end: same prompt, same schedule, same greedy commits,
compare the generated token sequences exactly.

The forward is already oracle-verified (run-bidir.sh, 7/7 variants at 0.00e0). What this adds is
the LOOP: block order, per-block step count, how many positions each step commits, which positions
win, and block-causal visibility. That is precisely where the engine was wrong before c4-739 — it
revealed 21 of a 32-token block from one forward, and let every block see every other.

Schedule, from chatllm.cpp models/bailing.cpp (LLADA2 registration):
  steps_per_block = min(steps, block_length)
  sched[i] = block_length / steps_per_block, remainder added to the LAST steps
  each step: forward, then commit the sched[i] highest-confidence still-masked positions IN THIS
  BLOCK, taking each position's argmax
Block-causal visibility is realised the same way the engine does it: the forward sees
prompt + committed blocks + the current block, and nothing after.

Usage: diffuse_ref.py <model.gguf> <n_fill> <steps> <block> [--ids]
"""
import sys
import numpy as np
import importlib.util

_spec = importlib.util.spec_from_file_location("llada2_ref", __file__.replace("diffuse_ref.py", "llada2_ref.py"))
ref = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(ref)


def diffuse(path, n_fill, steps, block, prompt=None):
    b, kv, tens, data = ref.read_gguf(path)
    mask = kv.get("tokenizer.ggml.mask_token_id")
    if mask is None:
        raise SystemExit("model carries no tokenizer.ggml.mask_token_id")
    # diffusion.shift_logits is written as a STRING by these converters ("true"/"false"), and
    # bool("false") is True in Python — the trap that made this reference disagree with the engine
    # by exactly one position (it emitted the LAST PROMPT token's argmax as the first fill token).
    # The engine reads it as `get_str(...) == "true"`; match that, and accept a real bool too.
    _sl = kv.get("diffusion.shift_logits", False)
    shift = (_sl == "true") if isinstance(_sl, str) else bool(_sl)
    if prompt is None:
        prompt = [1 + (i * 13) % 200 for i in range(12)]
    n_ctx = len(prompt)
    toks = list(prompt) + [mask] * n_fill
    masked = [i >= n_ctx for i in range(len(toks))]

    blk = block if block else max(n_fill, 1)
    n_blocks = (n_fill + blk - 1) // blk
    spb = max(1, min(steps, blk))

    for bi in range(n_blocks):
        lo = n_ctx + bi * blk
        hi = min(lo + blk, len(toks))
        blen = hi - lo
        if blen <= 0:
            continue
        sched = [blen // spb] * spb
        for i in range(blen % spb):
            sched[spb - 1 - i] += 1

        for s in range(spb):
            if not any(masked[lo:hi]):
                break
            # BLOCK-CAUSAL, matching the engine's default and chatllm itself. Truncation alone leaves the
            # prompt rows attending FORWARD into the current block, so from layer 2 the block's own rows
            # read activations the reference would never produce.
            lg = ref.forward(b, kv, tens, data, toks[:hi], blk, n_ctx)
            def at(i):
                return lg[i - 1 if (shift and i > 0) else i]
            cands = [i for i in range(lo, hi) if masked[i]]
            # confidence = p_top, the same quantity the engine ranks on
            def conf(i):
                z = at(i); z = z - z.max()
                p = np.exp(z); p /= p.sum()
                return p.max()
            take = sched[s] if s + 1 < spb else blen
            # Same deterministic rule as the engine: quantise confidence to 1e-6, ties go to the
            # LOWER index. Without this the comparison is meaningless on any block whose candidates
            # tie exactly, which is every fresh block.
            cands.sort(key=lambda i: (-round(float(conf(i)) * 1e6), i))
            for i in cands[:max(take, 0)]:
                toks[i] = int(np.argmax(at(i)))
                masked[i] = False

        if any(masked[lo:hi]):
            lg = ref.forward(b, kv, tens, data, toks[:hi], blk, n_ctx)
            for i in range(lo, hi):
                if masked[i]:
                    toks[i] = int(np.argmax(lg[i - 1 if (shift and i > 0) else i]))
                    masked[i] = False

    return toks[n_ctx:]


if __name__ == "__main__":
    path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/tiny-llada2.gguf"
    n_fill = int(sys.argv[2]) if len(sys.argv) > 2 else 16
    steps = int(sys.argv[3]) if len(sys.argv) > 3 else 8
    block = int(sys.argv[4]) if len(sys.argv) > 4 else 8
    out = diffuse(path, n_fill, steps, block)
    print(" ".join(str(t) for t in out))
