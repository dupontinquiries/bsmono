# Scout's streaming host copy is avoidable, and the code to avoid it is unreachable

## What the 50.6 GiB actually is

The refusal says it plainly:

> The engine requantizes every expert to its own q4 layout on load, which is the cost here.

And a few lines later, in the same message:

> **THIS CHECK RUNS BEFORE THE FILE-VIEW PATH IS ATTEMPTED**, so the figure assumes a host copy that
> may not be needed at all: when experts can be read as views into the mmap, `raw_expert_to_q4_resident`
> converts Q4_K straight to q4 **on the GPU** with no host copy and no f32 round-trip, exactly as the
> dense weights already do.

The dense path proves this works — every load prints
`Q4_K → q4 on GPU — 168 tensors converted in batch (no host copy, no f32 round-trip)`.

## The gap

```
build_moe_stream_from_weights(&[Vec<f32>], ...)   <- the ONLY caller. host f32 vectors.
build_moe_stream_raw(gate: &'static [u8], ...)    <- no callers
build_moe_stream_raw_typed(...)                   <- no callers
raw_expert_to_q4_resident(raw, ggml_type, n, k)   <- no callers
```

Three functions and a conversion entry point, all written, all reachable only from each other. The
streaming path takes `Vec<f32>` — which means **dequantize to f32 on the host, then requantize to q4**,
for every expert of every layer. That is the 50.6 GiB, and it is also a 20× transient per tensor
(1.58 bpw → 32 bpw → 4.5 bpw for IQ1_S; 4.5 → 32 → 4.5 for Q4_K, which is size-neutral at rest and
still round-trips through f32).

## Why it went unnoticed

The refusal message describes the better path in detail, so it reads as a known limitation being
worked around rather than as dead code. Someone reading it would reasonably conclude the file-view
path exists and does not apply here — and the message even says how to tell (`if no
file-views-unavailable line appears above this one, it was never tried`), which is a diagnostic for a
path that has no caller to reach it.

## Against the references

llama.cpp's `--override-tensor` / partial-offload streaming reads the file's native blocks and uploads
them; there is no host requantization step because the GPU kernels read Q4_K directly. Colibri holds
one canonical int8 format and does not stream at all.

So the shape here is llama.cpp's, and this engine has both halves of it — a GPU converter and a raw
streaming builder — with nothing joining them to the loader.

## What wiring it needs

The expert tensors must be readable as `&'static [u8]` views into the mmap, which
`mark_map_views_live` and `RawExpertSrc` already model. The loader currently dequantizes them into
`Vec<f32>` before calling the ring builder; it would instead pass the byte ranges and the ggml type.

The size prize on Scout: **50.6 GiB of host RAM becomes zero.** The experts stay in the mapped file and
convert on the GPU as the dense weights already do.
