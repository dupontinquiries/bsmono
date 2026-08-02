# Llama-4 Scout: what has been ruled out, and what has not

Scout loads with every tensor present and every shape matching, and writes nonsense. Recording the
state of the hypothesis list, because four commits of work eliminated less than they appeared to.

## Ruled out

**The NoPE interval is not file-controlled.** `no_rope_interval` is not a GGUF key — llama.cpp's table
defines no such thing and derives the interval from the architecture, as our constant does. Same for
`use_qk_norm` and the attention temperature `floor_scale`. The metadata reads added in _420/_421/_426
can never fire.

**The vision tower does not confuse the layer count.** Scout is multimodal and its 628 tensors include
a vision stack, but `n_layer` comes from `{arch}.block_count` in the metadata rather than from counting
`blk.*` tensors, so extra tensors cannot inflate it.

**The prompt template is translated correctly.** The Llama-4 branch of `normalize_prompt_template`
replaces `<|end_header_id|>` unconditionally, unlike the ChatML branch which only matched it followed
by two newlines — that partial replacement was a real garbling bug for ChatML models and is fixed.

**The chunked SWA rule is distinct from the standard one.** `(p1/n)*n` masks to the query's chunk
start, not the last *n* positions. Using Gemma's rule here would be wrong and is not what happens.

**The MoE routing prints what it does:** weight applied to the expert's output, 8 of 128 for Qwen —
and for Llama-4, `weight_before` with a raw sigmoid, which the arch vetoes metadata from overriding.

## Not ruled out

**The four constants themselves.** 4, on, 8192, 1 shared expert. They match Scout's documentation, and
nothing in this codebase can confirm they match Scout's *weights*. A wrong one degrades output with
every shape correct — the failure mode this whole list is about.

**Everything downstream of `--gate-bytes`.** If the invocation index is wrong on this driver, every
kernel indexing by `global_invocation_id` is affected, which is all of them. Scout's gibberish would
then not be a Scout bug at all, and neither would Q6_K's or Q4_K's.

## What would actually settle it

A **layer-by-layer comparison against llama.cpp on the same prompt and the same checkpoint**. Dump
`blk.0` output from both after a single token, diff, and walk forward to the first divergent layer.
That names the wrong constant directly instead of eliminating candidates one commit at a time.

It needs both engines on the same machine with the same file, which is a thing your setup can do and
this container cannot: there is no Scout checkpoint here, and lavapipe would not exercise the driver
where the divergence may live.
