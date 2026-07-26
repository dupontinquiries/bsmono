# Why `games/soldier` is fast on Asahi and the LLM decode is not

Both run on the same device through the same wgpu. The difference is not the amount of GPU work — it is
how the CPU hands that work over.

## What soldier does (games/soldier/src/gpu.rs)

```
init:    create bind groups ONCE (lines 383, 391, 476, 484); recreated only on resize (554)
frame:   queue.write_buffer(&self.ubuf, 0, ...)      // small uniform updates into PERSISTENT buffers
         let mut enc = device.create_command_encoder(...)
         enc.begin_render_pass(...)  x3               // several passes, ONE encoder
         self.queue.submit([enc.finish()]);           // ONE submit  (line 693 — the only one in the file)
         frame.present();
```

**One submit per frame. Zero allocations per frame. Zero bind groups created per frame.** Nothing is
read back. The CPU's job each frame is: write a few hundred bytes of uniforms, record passes, submit.

## What logscope decode does

Measured on Llama-4 Scout: **1152 submits per token, 48 polls per token**, `sync 3030 ms of 4399 ms`.

Two structural reasons:

1. **A readback per layer.** `moe_ffn_v1` downloads the router logits to route on the CPU. Each download
   drains the queue. 48 layers = 48 unavoidable pipeline stalls. A game never reads back mid-frame.
2. **Allocation per operation.** Every intermediate (`n1`, `q`, `k`, `v`, `attn`, `gu`, `hmid`, …) comes
   from the buffer pool. The pool recycles, so the same memory returns — but *which* buffer you get
   depends on free-list order, so the bind group differs run to run and the cache misses. Measured: 18%
   reuse before the view-keying fix in _220.

## The adoptable insight: decode is a FIXED-SHAPE pipeline

A game can pre-create bind groups because its per-frame resources never change shape. **Decode has the
same property and does not exploit it.** Every token runs identical work at identical shapes: `t=1`, and
`d`, `kv`, `ff`, `n_heads` are model constants. The tenth token allocates exactly the same sizes in
exactly the same order as the first.

So the game pattern applies directly:

* **Stable scratch, not a free list.** Hand out the same buffer for the same (call-site, shape) on every
  token — a linear frame allocator that RESETS rather than frees, which is what a game's per-frame arena
  is. Then every bind group is identical across tokens and the cache hits, instead of depending on
  free-list order.
* **Persistent uniforms.** Already partly done (`dims_cache`), and it is the same idea as soldier's
  `self.ubuf`.
* **One encoder, many passes.** Already done via frame batching (16 passes/submit). The limit is not the
  encoder — it is the readback, see below.

## What does NOT transfer

The per-layer readback is not a bad habit to be optimised away; it is what streaming REQUIRES. The CPU
must know which expert a token routes to before it can upload that expert's weights. A game has no
equivalent because it never asks the GPU a question mid-frame.

That is worth stating plainly because it bounds the win: adopting the game pattern removes CPU overhead
per dispatch, but **48 stalls per token remain** until either (a) enough experts are resident that no fill
is needed, so the v2 GPU-routed path can run and nothing leaves the GPU, or (b) routing is speculated and
verified. llama.cpp avoids the whole issue by keeping top-k in the graph — it does not stream experts.

## Priority

1. Stable decode scratch (this document's main point) — pure CPU-side win, bit-identical, bounded work.
2. Enough pinned experts to make the common layer fill-free, which then permits the sync-free path.
3. Everything else in the profile is smaller than these two.
