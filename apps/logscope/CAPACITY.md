# Capacity: stronger models on 64 GB at 10 instances

The fleet shares **one** resident copy of the weights (`Arc<Model>`); each concurrent
instance adds only its **KV cache**. So the memory question is:

    total ≈ weights(once) + N_instances × KV_per_instance(seq) + small overhead

KV bytes per generated/prefilled token, per instance:

    kv_bytes/token = 2 (K and V) × n_layers × (n_kv_heads × head_dim) × bytes_per_elem

`bytes_per_elem` is 4 for an f32 (Hot) cache, 1 for the int8 (Warm) cache the two-tier
`PrefixStore` demotes idle prefixes into.

## Per-model numbers (typical Llama-family GGUF configs)

| Model | layers | kv_dim | KV/token (int8 / f32) | weights Q4_K_M |
|-------|-------:|-------:|----------------------:|---------------:|
| 1B  (Llama-3.2-1B) | 16 |  512 | 16 KB / 64 KB  | ~0.8 GB |
| 3B  (Llama-3.2-3B) | 28 | 1024 | 56 KB / 224 KB | ~2.0 GB |
| 8B  (Llama-3.1-8B) | 32 | 1024 | 64 KB / 256 KB | ~4.9 GB |
| 14B (Qwen2.5-14B)  | 48 | 1024 | 96 KB / 384 KB | ~8.5 GB |

`kv_dim = n_kv_heads × head_dim`. All of these use 8 KV heads (GQA); the 3B/8B/14B use
head_dim 128 (the 1B uses 64). The engine reads every one of these from GGUF metadata —
validated bit-exact for a head_dim=128 GQA config in `kernel-validate`.

## 10 instances at seq 8192, int8 KV

| Model | weights | 10 × KV(int8,8k) | total | fits 64 GB? |
|-------|--------:|-----------------:|------:|:-----------:|
| 1B  | 0.8 GB | 1.3 GB  | ~2.1 GB  | trivially |
| 3B  | 2.0 GB | 4.5 GB  | ~6.5 GB  | easily |
| 8B  | 4.9 GB | 5.1 GB  | ~10 GB   | easily |
| 14B | 8.5 GB | 7.7 GB  | ~16 GB   | yes, wide margin |

The headline: **at 10 instances, 64 GB is not the constraint** — even a 14B leaves you
~48 GB free. That budget buys you any one of:

- **Higher-precision weights**: run the 8B at f16 (~16 GB) with 10 × f32 KV @ 8k (~20 GB) → ~36 GB.
- **Much longer context**: 8B Q4_K_M with 10 instances at **32k** int8 KV → ~25 GB.
- **A bigger model**: 32B Q4_K_M (~19 GB) + 10 × ~1 GB KV → ~29 GB; even 70B Q4_K_M
  (~40 GB) + ~13 GB KV ≈ 53 GB fits, though see the caveat below.

## The real limiter is throughput, not memory

One GPU decodes one matmul stream at a time. As you stack instances they share that stream,
so **per-instance tok/s falls even while memory is half-empty**. Memory tells you what *fits*;
`--loadstep` tells you what *performs*:

    kernel-validate --loadstep /models/llama-3.1-8b-instruct-q4_k_m.gguf

Find the N where per-instance tok/s crosses 1/2 of baseline — that's your comfortable
stacking limit, usually reached well before the memory limit on a model ≥ 8B.

## Recommendation

For a maintenance/log tool the sweet spot is **8B Q4_K_M**: a clear quality jump over the
1B, ~10 GB total at 10×8k (GPU and RAM both have room), and it loads through the same path
the 1B does. Step up to **14B** when answer quality matters more than latency; stay at
**1–3B** when you want many more than 10 instances or very long contexts per instance.
Run `--loadstep` on the actual box to set the instance cap, then size context from the
table above against whatever RAM is left.

## Engine support status

- Loader is fully config-driven (block_count, head_count, head_count_kv, embedding_length,
  feed_forward_length, rope_freq_base, head_dim). Validated for GQA and head_dim=128.
- Quantization read paths: Q4_0, Q4_K, Q6_K, Q8_0 (dequant kernels validated bit-exact).
- `PrefixStore` demotes idle shared prefixes f32 → int8, stretching effective KV capacity
  beyond the table above when prefixes are reused across instances.
