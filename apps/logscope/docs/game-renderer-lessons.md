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

## Comparing four codebases in this repo — the variable is submits per unit of work

| codebase | submit pattern | outcome |
|---|---|---|
| `games/soldier` | 1 `queue.submit` in the whole renderer, once per frame | smooth |
| `games/clawbound` | 4 submit sites total | smooth |
| `games/unendlich` | 58 sites; `neural_gemm_pipeline.rs` does submit-then-wait-for-fence **per dispatch** (its own comment calls it "harness pattern") | **stutters** |
| `logscope` decode | 1152 submits/token (before the staging ring) | slow |

Unendlich is the useful data point precisely *because* it stutters. It is the game doing the most GPU work
**and** the only one submitting per dispatch — it fails the same way inference did, for the same reason.
That makes the diagnosis an observation rather than a theory: the three smooth programs batch, the two slow
ones don't.

### What unendlich does well and inference still does not

Both are worth taking, and neither is done yet:

* **Push constants** (`cmd_push_constants`) for per-dispatch parameters, instead of a uniform buffer.
  logscope passes `dims` through a uniform buffer, which means a buffer per distinct dims value and a
  dims-buffer handle inside the bind-group cache key. Push constants would remove both. Needs
  `wgpu::Features::PUSH_CONSTANTS`, which is not universally available — so it is an opt-in path, not a
  replacement.
* **One persistent descriptor set per layer**, updated once at setup ("the descriptor set must already be
  updated" — its dispatch precondition). Our equivalent is the bind-group cache, which is reactive:
  it caches what was asked for. Building the layer's bind groups once at load would make the hit rate
  100% by construction rather than by luck.

### What unendlich does badly, and the lesson

`submit → wait_for_fences(u64::MAX)` per dispatch drains the pipeline every time. It is the right shape for
a correctness harness and the wrong one for a frame budget. The equivalent mistake in inference was two
`end_frame()` calls per expert fill (_181), which was correct for the same reason and wrong for the same
reason — and cost 1152 submits/token until a ring replaced it.

## Measured: the per-token arena did NOT do what it was built for

The arena (`scratch_begin`/`scratch_end`) was built on the theory that the buffer pool's free-list order
made bind groups differ between tokens, and that stabilising buffer identity would lift the 18% reuse the
profile reported.

**That theory is wrong, and `--gate-scratch` now measures it.** On a fair test — several interleaved shapes,
inputs pre-uploaded so a freshly created input buffer cannot mask the effect — bind-group reuse is
identical with the arena on and off:

```
bind-group reuse: arena off 83% (60/72) → arena on 83% (60/72)
```

The pool is already deterministic when the alloc/free pattern repeats, which is exactly what decode does.

So the real cause of 18% reuse was **not** buffer identity. It was sub-buffer **views** bypassing the cache
entirely (the old key skipped any input with a view, and streaming expert weights are all views). Keying on
the view span fixed that. The arena addressed a problem that was not there.

**It has since been REMOVED**, for a second reason that only appeared under the full kernel suite: the arena
stored its buffers in a `thread_local`, and the "threaded workers (shared Gpu/Model)" check spawns threads.
When a spawned thread ends, TLS destruction drops **wgpu buffers**, which panics inside a destructor — a
non-unwinding panic, so the process aborts. Validation died at check 112 with *"panic in a destructor during
cleanup"*.

Guarding the *access* with `try_with` did not help, because the problem is the destructor of the
thread-local's *contents*, not reads of it. The alternatives were to move the storage into `Gpu` behind a
mutex (wrong: two threads sharing one arena would interleave allocations and hand out each other's buffers)
or to key it by thread id (complexity for a thing already measured as worthless). So it was deleted.

Three lessons:

* The game comparison correctly identified *submits per unit of work* as the variable, and the staging ring
  (1152 → 72 submits/token) came out of it. That part held up.
* "Games pre-create bind groups, so buffer churn must be our problem too" was an inference from a working
  system to a broken one, and it did not survive measurement. Adopting a pattern is not the same as having
  the problem the pattern solves.
* A thread-local is the obvious way to make per-thread state, and it is the wrong container for GPU
  handles: their destructors run during thread teardown, where a panic cannot unwind. The existing
  `ArenaGuard` already carried a comment about not touching the GPU during teardown — the same trap, one
  layer down, and it was not read carefully enough before adding a second one.

## CORRECTION: the staging ring did not reduce Scout's submits, and my attribution was wrong

Measured on hardware after the ring landed: **1151 submits/token, unchanged.**

The arithmetic that produced the ring — `1152 / 48 layers = 24 = 12 fills x 2 end_frame()` — was a
coincidence. Scout does not take the path those `end_frame()` calls live in. Its expert tensors are Q6_K,
so `MoE file-views unavailable` fires and it uses the **host-copy** fill path, where `StreamSlots::fill` is
a bare `queue.write_buffer` with no submit at all. The ring optimised a path this model never executes.

Two things follow, and the second matters more than the first:

* The ring is still correct and still helps the file-view path (any all-Q4_K MoE checkpoint), so it stays —
  but it is not the fix for the number in the profile.
* **A plausible arithmetic match is not evidence.** 1152/48 = 24 and 12x2 = 24 was too neat, and I did not
  check that the model in question reaches that code before acting on it. The check would have been one
  grep: does Scout hit the `raw` branch? It does not, and the loader says so in its own log line, three
  lines above the profile I was reading.

The real source of ~24 submits/layer on Scout is still unidentified. `fill` does not submit; the router
readback accounts for one per layer.
