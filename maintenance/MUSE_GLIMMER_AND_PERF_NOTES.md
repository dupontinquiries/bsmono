# Muse Glimmer — text-only, from llama.cpp PR #26841

Read off `62bf73d25 model: Muse Glimmer Support (#26841)` in the upstream
llama.cpp history (the whole PR is squashed into that one commit, so the
diff is complete and self-contained). Everything below is what that
implementation actually does, not a guess from `config.json`.

## Yes — it loads without vision, and the split is already clean

The PR touches 22 files. The division is total:

| text model (needed) | vision (skippable) |
|---|---|
| `src/models/muse-glimmer.cpp` (208 lines) | `tools/mtmd/models/muse-glimmer.cpp` (88) |
| `src/llama-arch.{h,cpp}` (1 line each) | `tools/mtmd/clip.cpp` (+91) |
| `gguf-py/gguf/constants.py`, `tensor_mapping.py` | `tools/mtmd/mtmd-image.{h,cpp}` (+68) |
| `conversion/muse_glimmer.py` — `MuseGlimmerModel` | same file — `MuseGlimmerVisionModel` |

Vision ships as a **separate mmproj GGUF**. llama.cpp loads the text model
with no knowledge of it; mtmd is a different tool linking a different
model file. So the staged plan in C13_TODO §38 is not just viable, it is
the shape upstream already has — and the text half is genuinely small.

Revision to §38: **the text model is much less exotic than config.json
suggests**, because the converter folds most of the oddities into weights
before the engine ever sees them. Details below.

## The engine only has to implement five things it does not already have

Everything else is a standard dense SwiGLU transformer with GQA.

### 1. Per-layer RoPE on/off — SWA layers rotate, full-attention layers do not

```cpp
const bool use_rope = hparams.is_swa(il);
```

That single line explains the `layer_rope_theta` array with zeros in it.
It is **not** per-layer theta values needing separate tables — it is
NoPE on the 13 `full_attention` layers and RoPE on the 39
`sliding_attention` ones. The converter writes it as a bool array:

```python
add_sliding_window_pattern([t == "sliding_attention" for t in layer_types])
```

**Cross-check that the pattern is right.** The loader prefers an explicit
bool array from the GGUF and falls back to a period. Upstream's period
rule is `is_swa[il] = (il % n_pattern < n_pattern - 1)` with a default
`swa_period = 4`. For 52 layers that yields 3 sliding + 1 full per group
of 4 = **39 sliding, 13 full** — exactly the counts in this checkpoint's
`layer_types`. So the array and the period agree here, which is a free
assertion to write into our loader: if a file carries both, they must
match, and if they disagree the array wins (upstream's order) and we say
so rather than silently picking one.

There is a separate `rope_freq_base_train_swa` (falling back to the main
base) if the two ever differ; on this checkpoint both are 500000.

For our engine this is per-layer dispatch on an existing axis. The Gated
DeltaNet routing in `decode_step_full` is the precedent — same shape,
one flag per layer, chosen at load.

### 2. Sliding-window attention with a 2048 window on those 39 layers

`build_attn_inp_kv_iswa()` — an interleaved SWA/full KV cache. This is
the one genuinely new *mechanism*, and it is the piece worth checking
against what we already do: our KV cache is uniform per layer. SWA layers
only need a 2048-position window, which is a memory saving as well as a
correctness requirement.

### 3. Attention output gate (sigmoid), between SDPA and o_proj

```cpp
gate = wqkv_gate @ attn_inp        // from the PRE-attention hidden state
...
cur  = build_attn(...)             // wo deliberately NOT applied here
gate = sigmoid(gate)
cur  = cur * gate
cur  = wo @ cur
```

Note the ordering: the gate is projected from the input to the attention
block, applied to the attention output, and only then does `wo` run.
Upstream says this matches `afmoe`. New tensor: `attn_gate` at
`{n_embd, n_embd_head_k * n_head}`.

### 4. Sandwich norms — post-attention and post-FFN, at a different epsilon

```cpp
const float post_norm_eps = 1e-8f;   // hardcoded; f_norm_rms_eps is 1e-5
```

Applied after attention and after FFN, before each residual add. The PR
history is instructive here: it went from a new GGUF param to
"Hardcode post_norm_rms_eps instead of new param". We should hardcode it
too rather than inventing a metadata key that no converter writes.

Also: a **weightless RMS norm on the token embeddings** before layer 0
(`build_norm(inpL, nullptr, nullptr, LLM_NORM_RMS, -1)`), added late in
the PR as "Apply norm after token embeddings".

### 5. Output multiplier, then tanh softcap

```cpp
cur = output @ cur
cur = cur * f_logit_scale                      // output_multiplier 0.196…
if (f_final_logit_softcapping) {               // 20.0
    cur = tanh(cur / cap) * cap
}
```

The softcap is lifted from `gemma3.cpp`, so if we already have Gemma
softcapping it is the same code.

## What we do NOT have to implement — the converter absorbs it

This is the part that changes the cost estimate, and it is invisible from
`config.json`:

- **`qk_scale_factor` (3.87) does not exist at runtime.** The converter
  *synthesizes* QK-norm weights that carry it: `q_norm` is filled with
  the constant `qk_scale_factor` across `head_dim`, and `k_norm` is
  filled with **ones**. The model does scaleless RMSNorm followed by a
  scale; the converter turns that into ordinary weighted RMSNorm. So we
  need Q/K norm (which we already have — `has_qk_norm`) and nothing else.
- **`weight + 1` on norms** is folded at conversion (`norm_shift`: +1 on
  the four layer norms, 0 on the final norm). Gemma-style, already
  handled by whoever converts.
- **Q/K permutation.** The converter inverts transformers'
  `_permute_for_rope` so the GGUF is in ggml's interleaved layout. Our
  loader already reads `rope_interleaved` vs NEOX split-half from the
  file; nothing new, but worth knowing the GGUF is *already* unpermuted
  and must not be permuted again.

## Concrete next step

The text path is: existing dense transformer + (1) per-layer RoPE flag,
(2) SWA windowed KV on flagged layers, (3) sigmoid attention gate,
(4) sandwich norms at 1e-8 + embedding norm, (5) logit scale + softcap.
Four of those five are small arithmetic; only SWA touches cache layout.

Refuse image/video tokens (`image_token_id` 200092, `video_token_id`
200091) **by name** at tokenize time, so a multimodal prompt fails
honestly instead of producing fluent nonsense from spliced-in garbage.

---

# Performance lessons from the same history

Mined for the "what should we look out for" question, so we can add tests
for the ones that apply instead of building harnesses to rediscover them.

## Directly relevant to our Asahi/Vulkan memory wall

The run log shows the arena hitting a wall at 24 GiB of mapped slabs
against a compiled-in 31 GiB clamp, with staged writes deferring but not
removing it. Two upstream commits bear on exactly this:

- **`32120c10e` vulkan: prefer host-visible memory buffers on UMA
  devices.** The M2 Ultra under Asahi *is* a UMA device. On UMA there is
  no separate VRAM to stage into — a device-local, non-host-visible
  buffer is a pointless copy of memory the CPU could have written
  directly. If our arena is choosing device-local slabs and then staging,
  we are paying a copy AND hitting an allocation class that is scarcer
  than it needs to be. Worth checking which memory type our slabs
  actually request.
- **`558e221b7` vulkan: record actual memory properties during buffer
  creation** — a one-line fix. The driver may not give you the memory
  type you asked for; recording what you *requested* rather than what you
  *got* makes every downstream accounting decision wrong. Our budget
  bookkeeping should record the properties the allocation actually came
  back with. This is the same class as the "constant, not a measurement"
  problem already flagged in C13_TODO §37.

## Patterns worth watching for, with the commit that shows each

- **Row-level instead of per-element copies** — `369e1cd61` (concat op,
  per-element memcpy → row-level). Any loop doing element-wise copy in a
  hot path is this bug.
- **Tune the mat-vec/mat-mat crossover per hardware AND per quant type** —
  `2b5621094` (CUDA), `2cdae802e` (Vulkan, Strix Halo). Our autotuner
  tunes tiles; the *crossover point* between decode-style matvec and a
  full GEMM is a separate knob and is quant-dependent. We hardcode one.
- **Check shared-memory limits before selecting a shader** —
  `706fbd8ab`, `d6d0ce821` (reduce iq1 shared memory for mul_mm). A
  shader that exceeds the device's shared memory fails at dispatch, which
  on our stack would look like the "device error (non-fatal, logged)"
  entries in the log.
- **Non-contiguous KV restore is worth optimizing** — `2d8d612e4`. Matters
  once we do prefix reuse / trie sharing.
- **Fuse post-scale into the matmul** — `3899b39ce` (MMVQ post-scale for
  NVFP4). We already do int8-resident transcode without an f32
  round-trip; the same idea applied to the dequant scale is the next one.
- **Fusion must check for memory overlap first** — `3769fe6eb`, and
  `b387ddfd8` (missing view-alias dependencies in graph_optimize). Both
  are aliasing bugs in fusion passes. Our gate‖up fusion slicing is
  exactly this shape and deserves a test that two fused views cannot
  overlap.

## Tests worth adding once the above is understood

Cheap, pure, no GPU:

1. SWA pattern parse: a bool array of 52 entries → 39 sliding / 13 full,
   and the per-layer `use_rope` flag matches it exactly.
2. Softcap arithmetic: `tanh(x/c)*c` is monotonic, bounded by ±c, and
   an absent/zero cap is the identity.
3. Norm epsilon provenance: post-norm uses 1e-8 where layer norm uses
   `f_norm_rms_eps`, and mixing them up changes the output — pin it so
   a future refactor cannot quietly unify the two constants.
4. Attention-gate ordering: gate is computed from the pre-attention
   input, not the attention output, and `wo` runs last. Easy to get
   backwards and it produces plausible-looking garbage.
5. Memory-property recording: whatever we record for a slab equals what
   the allocation returned, not what was requested (§37's theme).


---

# Follow-up: what the UMA lesson turned out to mean here

`32120c10e` (prefer host-visible buffers on UMA devices) reads like a
direct hit on our arena, but checking the code first changed the answer,
so recording it rather than leaving the suggestion standing:

**The arena has already tried the mapped/unmapped axis and it is not the
discriminator.** The comments at the slab allocator record three runs at
different slab sizes, then the unmapped fallback, and conclude: "the
device will not hold more than ~31 GiB of buffers at once, however they
are created." So it is not a mapping-class problem, and llama.cpp's fix
does not transfer as stated — wgpu does not expose Vulkan memory-type
selection the way ggml-vulkan does, so there is no equivalent knob to
turn.

What DID transfer is the sibling commit, `558e221b7`, and it transferred
completely: record what you observed where you were recording what you
assumed. That is now done — see the `concurrent_ceiling_gib` commit. The
clamp prefers a stored per-adapter measurement over the compiled-in 31.0
and prints which one it used.

**The trap found while doing it, worth remembering:** there were already
two uncalled measurement stores, and they hold DIFFERENT quantities.
`store::vram_{get,put}` holds the single-allocation ceiling (63 GiB
measured here); the clamp needs the concurrent ceiling (~31 GiB on the
same machine). Wiring the obvious one into the obvious place would have
doubled the budget and reintroduced the mid-quantisation death the
constant exists to prevent. Both are "the vram number" in casual speech
and they differ by 2x. A test now pins the distinction.

Open question for whoever has the device: **which quantity does
`--gate-vram` actually measure?** It inflates with slabs and keeps them,
which sounds concurrent, but the arena comments say the probe "allocates
and does not keep its buffers mapped", and it reported 63 where the
concurrent wall is 31. Until that is settled, `--gate-vram` deliberately
does not write the concurrent store — an auto-wire on the wrong quantity
is worse than the constant, because the constant is at least known to be
conservative.


---

# Implementation status (revised twice, both times downward)

Registering the arch and checking `archfeat::implemented()` rather than
assuming changed the estimate again. **Five things to build became three.**

Already shipping, so NOT gaps: `StandardSwa` (sliding-window attention),
`PostNorm` (the post-attn/post-FFN sandwich norms), `NoPe` (per-layer
no-positional-encoding — the axis Llama-4 iRoPE uses, which is exactly
how Glimmer's full-attention layers behave), `QkNormPreRope`,
`FinalLogitSoftcap`. Most arrived with the Gemma family.

Genuinely missing, now registered as gaps so the loader refuses by name:

1. `SeparateAttnOutputGate` — deliberately NOT folded into the existing
   `GatedAttentionOutput`. That one takes the gate out of a double-width
   q (interleaved per head); Glimmer projects its own `attn_gate` from
   the pre-attention hidden state. Same idea, different tensor, extra
   matmul.
2. `EmbeddingNorm` — weightless RMS on embeddings, not Gemma's ×sqrt(d).
3. `OutputMultiplier` — the `output_multiplier` scale before the softcap.

Landed alongside: `archcfg::swa_pattern()` handling both upstream
encodings, with the 39/13 cross-check now an assertion rather than an
observation.

**Still to build for a text-only load:** the three features above, plus
loading the per-layer `attn_gate` tensor, plus refusing image/video
tokens (200092 / 200091) by name at tokenize time so a multimodal prompt
fails honestly instead of splicing garbage into the text stream.


---

# The submit/pass finding, from llama.cpp's own WebGPU backend

Our measured bottleneck is not bandwidth. The field run says so directly:
`19 GB/s of 800 spec = 2% of the decode ceiling; NOT bandwidth-bound`,
`sync 87760ms of 90710ms`, `7800 submits (7/tok)`, and
`96% of submits were forced by PASS BOUNDARIES ... 115100 passes for 153438
dispatches` — **1.3 dispatches per compute pass**.

llama.cpp has a WebGPU backend, i.e. the same wgpu-shaped API we target, and
it is the closest thing to a reference implementation available. What it does:

* `ggml_backend_webgpu_graph_compute` opens **one compute pass** with
  `BeginComputePass()` and accumulates dispatches into it.
* It breaks the pass on exactly one condition:
  `num_batched_kernels >= command_submit_batch_size`, where the default is
  **`WEBGPU_DEFAULT_COMMAND_SUBMIT_BATCH_SIZE = 64`** (`ggml-webgpu.cpp:89`).
  That is a throughput knob, not a correctness constraint.
* `82677a6ed` is the commit that introduced this ("compute pass batching and
  removing profiling overhead"), and its own changelog ends with **"remove iOS
  throttling now that we're batching compute passes"** — batching removed the
  need for a throttle they had previously had to add.
* `54121f732` later made it toggleable, and the un-batched path (one pass per
  dispatch — our current shape) exists **only for GPU profiling**.

So the arrangement we are in by default is the one upstream uses only when it
wants per-dispatch timestamps.

## The part that deserves scrutiny, stated carefully

**There is not one mention of the word "barrier" in the entire 4790-line
WebGPU backend.** They batch 64 dispatches into a pass, those dispatches read
and write the same storage buffers, and nothing in the backend reasons about
synchronising between them. That is only sound if WebGPU guarantees hazard
tracking between dispatches within a compute pass — which the spec does, and
which is a real difference from raw Vulkan, where the barrier would be the
caller's job.

If that reading is right, it has a specific consequence for us:
**`LOGSCOPE_PASS_FUSE` may have been retracted for the wrong reason.** It was
withdrawn because it "broke AR decode 6/6 (c4-736) and changes generated
tokens on 3 of 6 diffusion configurations". If the API guarantees ordering
within a pass, then merging dispatches into one pass cannot itself be the
corruption — and the actual bug would be something the fusion did *alongside*
the merge. The bind-group cache is the first suspect, since the same run
reports `76762 of 115100 lookups hit ... Repeat misses: 357 output / 34142
dims / 4109 inputs`: a fused pass that reuses a bind group whose dims buffer
has since been rewritten would produce exactly the symptom (right shape,
wrong numbers, non-deterministic across configurations).

### Corrected, and it is better than the hypothesis

Reading our own retraction comment afterwards (`lib.rs::pass_fuse_enabled`)
shows the analysis had already got further than I had, and got it right:

> the leading candidate is dims-buffer lifetime — deferred dispatches replay
> after `ring_dims` has recycled and rewritten the uniform they read, so an
> earlier dispatch executes with a later dispatch's dims.

That is a RESOURCE-LIFETIME bug, not an ordering bug, and it is consistent
with everything above: WebGPU's intra-pass guarantee covers ordering, so
batching was never going to corrupt by itself. My guess at the bind-group
cache was wrong and our own bisection had already cleared it ("BG cache,
FUSE_GLU and FUSE_DOWN are each clean, LOGSCOPE_PASS_FUSE=1 alone reproduces
it").

**And upstream shows the fix directly.** `initialize_webgpu_context` sizes
its parameter arena as:

```
command_submit_batch_size + WEBGPU_NUM_PARAM_SLOT_SAFETY_MARGIN   // 64 + 10
```

The param pool is sized *by the batch size*, so a slot cannot be recycled
while a dispatch in the current batch still refers to it. That is precisely
the invariant our `ring_dims` violates when fusion defers dispatches: our ring
is sized independently of how many dispatches may be in flight, so a long
fused batch wraps the ring and overwrites uniforms that pending dispatches
have not consumed yet.

So the fix is not "make fusion safe" in the abstract. It is: **size the dims
ring to at least the maximum fused batch length, plus a margin** — and then
the wrap can no longer reach a live slot. That also explains why the
immediate-encode path is fine (it consumes each slot before the next is
handed out) and why only the deferred path breaks, which the retraction
comment noted but could not account for.

**Still to prove before re-enabling.**
`LOGSCOPE_PASS_FUSE` stays retracted until someone has:

1. sized `ring_dims` to `max_fused_batch + margin` (upstream's shape), and
2. re-run `gate-decode-parity` — the probe that actually caught this, because
   it compares a fused DECODE against the unfused PREFILL that must agree with
   it. `gate-passfuse-diffusion.sh` missed the bug precisely because it
   compared fused against fused, and our own notes already say so: "a gate
   that only exercises the feature it is testing cannot see the feature being
   wrong".

If the ring is the mechanism, parity should return to the 8e-8 agreement the
unfused path gets. If it does not, the mechanism is still unproven and the
retraction stands.

Payoff if it holds: 96% of submits are pass boundaries, and sync is 97% of
wall time. Going from 1.3 dispatches/pass toward upstream's 64 is the largest
single lever in the engine, and it is worth more than any kernel change on
this list.

## Smaller WebGPU-backend findings worth copying

* `7c908502e` — "improve MTP inference by using mat-vec path for small
  batches". We have `MATVEC_SMALL_Q4` already; the question is whether our
  crossover threshold is tuned or assumed. Upstream tunes it per quant type
  (`2b5621094` does the same for CUDA), and ours is one constant.
* `bd90fc74c` — flash attention improved by *software pipelining*. Relevant
  because our phase-balance line says decode has "nothing in flight to cover
  memory latency", which is the same disease pipelining treats.
* `32e41fa5b` — "tune subgroup split (d_split) in flash_attn_vec". We disable
  subgroups entirely on Asahi for a real hang, so this is not directly
  usable — but it is a reminder that the subgroup path is where upstream gets
  its wins, and our hang is worth revisiting rather than treating as settled.
* `03ea04175` / `ef07a4090` — set_rows and rms_norm/soft_max optimisations,
  both small and self-contained. Good candidates if the pass work turns out to
  be blocked.
