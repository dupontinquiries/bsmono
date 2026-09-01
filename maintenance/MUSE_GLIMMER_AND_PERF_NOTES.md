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
