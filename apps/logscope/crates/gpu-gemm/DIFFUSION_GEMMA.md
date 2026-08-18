# DiffusionGemma (google/diffusiongemma-26B-A4B) — what is pinned, what is interpreted, what is missing

Block-diffusion generation on a Gemma 4 MoE stack. This file records the architecture facts the
converter and the CPU reference stand on, with sources; the places where the public drop is
silent and an INTERPRETATION had to be stated; the memory arithmetic; and what "green" currently
means (a CPU oracle in a sandbox — no GPU, no real weights, `archcfg` keeps `supported: false`).

Sources, in trust order: the HF drop's `config.json` / weight index / `README.md` /
`scheduler/scheduler_config.json` / `generation_config.json` (modeling code is **NOT in it** —
it targets `transformers 5.8.0.dev0`); `llama.cpp` `gemma4.cpp` + `llama-graph.cpp` for the base
Gemma 4 stack conventions; the drop's `tokenizer.json`. Where the two disagree with a guess of
mine, the gate that caught it is named below.

## The layer plan

30 layers on a **5-sliding : 1-full** pattern, full LAST in each group of 6 → full layers are
exactly `[5, 11, 17, 23, 29]` — and the weight census agrees: precisely those five ship **no
`attn_v`**. Everything that matters differs per layer type:

| | sliding (25 layers) | full attention (5 layers) |
|---|---|---|
| head_dim | 256 | **512** (`global_head_dim`) |
| KV heads | 8 | 2 |
| rope θ | 1e4 | 1e6 |
| rotary | full head | **partial 0.25** → n_rot 128 |
| window | 1024 | — |
| V source | own `v_proj` | **the RAW `k_proj` output** |
| KV width (K or V, per token) | 2048 | 1024 |

16 query heads both kinds; d = 2816; vocab 262144, tied embeddings, embeddings scaled ×√d on
input. V is weightless-rms-normalized per head in BOTH cases (gemma4.cpp norms V
unconditionally); on full layers V comes from the raw K projection while K separately gets
`k_norm` + rope, so the cached K and V still differ. QK-norm (per-head, shared per-dim weight,
Gemma `(1+w)` form) runs BEFORE rope. Attention scores are **UNSCALED** — gemma4.cpp sets
`f_attention_scale = 1.0`; QK-norm makes 1/√d redundant. The converter writes
`attention.scale = 1.0` explicitly because the engine's generic loader applies the kernels'
1/√head_dim default when the key is absent, and a wrongly scaled softmax is a quietly-worse
model, never a visibly broken one.

## The twin FFN (per layer)

Two PARALLEL branches off `attn_out` (attention output + residual), joined by one post-norm:

```
mlp = post_ffw_norm_1( GeGLU_dense( ffn_norm(attn_out) ) )          # shared dense, ff 2112
moe = post_ffw_norm_2( MoE( pre_ffw_norm_2(attn_out) ) )            # 128 experts top-8, ff 704
out = ( post_ffw_norm(mlp + moe) + attn_out ) × layer_scalar
```

Router input is a **weightless rms of `attn_out`** (not the branch input) divided by √d and
multiplied elementwise by a learned scale VECTOR (`ffn_gate_inp.scale`); gating is softmax over
all 128 with top-8 renormalized; each expert's output is multiplied by its entry in a learned
per-expert scale vector (`ffn_down_exps.scale`). Activation is `gelu_pytorch_tanh` everywhere.
The fused expert stack `ffn_gate_up_exps` is HF's `gate_up_proj` copied verbatim: **GATE is the
first `n_ff_exp` rows** of each expert slab (llama-graph.cpp splits the merged output with the
gate view at offset 0). Final logits get the Gemma-2-style tanh softcap at 30.0.

Two SETS of per-layer output scalars exist: `blk.N.layer_output_scale` for the denoise pass and
`blk.N.layer_output_scale_enc` (from `model.encoder.language_model`) for the encode pass. The
reference REFUSES an encode pass when the enc set is absent rather than borrowing the decode
one — borrowing runs and is quietly wrong, the worst available failure mode.

## The block-AR denoise loop

Canvas 256 refined in blocks of 32 (`canvas % block == 0` is enforced at convert time AND by
`read_shape`). Per block: bidirectional denoise over the canvas against the causally-encoded
prefix cache, then the committed block is ENCODED (causal, enc scalars) into the cache, then the
next block. `diffusion.shift_logits` is `"false"` — in-place LLaDA lineage, row i predicts
position i — written as an EXPLICIT STRING because the engine reads it with `get_str` and an
absent kv falls to a lineage default (the llada fork-GGUF lesson).

Scheduler (BlockRefinementScheduler as shipped): per step, positions at/above the 0.95
commit threshold commit outright; remaining still-masked positions are taken
lowest-entropy-first while the running SUM of their entropies stays ≤ 0.1; if nothing commits,
`minimal_topk` (1) forces progress; **everything uncommitted renoises to the mask token** —
explicitly, so a draft can never linger. Temperature anneals 0.8 → 0.4 linearly over ≤ 48
steps. Adaptive stop requires BOTH at once: mean entropy over the WHOLE canvas (committed rows
included) below 0.005 AND the canvas argmax identical across consecutive reads (stability 1).
Hitting the step cap FLUSHES remaining positions as argmax — emitting mask tokens into a reply
is never the right reading of "out of steps".

## Interpretations, stated (the drop is silent; these are choices, each isolated in ONE place)

1. **"Mutual information bound"** → implemented as sum-of-selected-marginal-entropies ≤ 0.1
   (the parallel-decode joint-vs-independent gap is bounded by that sum). One function
   (`blockrefine::plan_step`) changes if the reference turns out to use a different functional.
2. **"Proportional" rope** on full layers: no modeling code, no `rope_freqs` tensor. The
   converter fabricates nothing; the CPU reference runs plain partial-rotary NEOX at θ=1e6 —
   correct where a factor of ~1 is the only sane reading, unverifiable past that. First thing
   to wire when transformers 5.8 or llama.cpp lands the formula.
3. **Sliding window under bidirectional attention**: the card is silent; `|Δpos| < window` in
   either direction is the generalization implemented. The shipped model cannot tell (window
   1024 > canvas 256); the fixture can, which is why the rule is one function (`visible`).
4. **Self-conditioning** (`self_cond.*` FFN at the decoder top): weights load, presence is
   gated, semantics are UNKNOWN — the forward does not apply it and says so once. Applying it
   with guessed semantics would run and be wrong.

## Tokenizer

No `tokenizer.model` ships — only the 32 MB HF `tokenizer.json`, and it declares **model type
BPE**: Gemma's SentencePiece model is BPE-TYPE, merges carry the whole segmentation, no unigram
scores exist to lose. That makes native GGUF emission COMPLETE from the json (262144 pieces,
514906 merges) — see the converter README for the emission + probe workflow.

Facts pinned against the real file:
* The ENTIRE normalizer is one rule: space → `▁`. **No dummy-prefix prepend** (unlike typical
  SP setups). The `Split(" ")` pre-tokenizer is a no-op post-normalization.
* The post_processor adds NOTHING — no BOS. `add_bos_token = false` in the emission; BOS
  placement is the chat template's job.
* `<mask>` is id 4; specials 0–4 are pad/eos/bos/unk/mask; 24 added tokens total including
  `<|turn>`/`<turn|>` at 105/106 and `<|tool…>` families.
* **Donor hazard, on record**: a Gemma-3 donor GGUF passes the 262144-width check while
  shipping DIFFERENT special strings at the same ids (105/106 are
  `<start_of_turn>`/`<end_of_turn>` there). Width cannot see that; every chat marker would
  tokenize wrong and the failure would read as "the model is bad at conversation". Native
  emission makes the donor path unnecessary.
* Byte fallback (`<0xNN>`) barely fires at 262k — `é` and `☕` are vocab pieces; U+13000
  (Egyptian hieroglyph) is the probe that actually exercises it.
* The HF reference's `decode` SKIPS specials by default; the engine decodes them literally —
  which is why engine-side `decode(ids) == text` holds for every probe including `a<mask>b`.

## IQ1_S geometry

Expert `gate_up` rows are 2816 wide (2816 = 11×256 → IQ1_S-alignable) but expert `down` rows
are **704 wide** — NOT a multiple of the 256-weight IQ1_S block, so ggml itself cannot quantize
them to IQ1_S and a real "IQ1_S" file of this model is **necessarily a per-tensor mix**. The
engine prices files per tensor (`store::estimate_resident_for_load` walks the tensor table with
no arch knowledge; 256-aligned type-19 tensors are priced at the file's own bytes exactly when
the loader will hold them natively, `LOGSCOPE_NO_IQ1_NATIVE` flips both together), and
`Iq1Weight::row_bytes(704) == None` is the alignment gate, not a bug.

## Does it fit? (shape arithmetic — the estimator is the authority once a file exists)

Parameter census from the shape (embed tied, counted once):

```
embed                262144·2816                        =  0.74 B
per sliding layer    attn 34.6M + dense 17.8M + experts 761.3M ≈ 813.7M   (×25)
per full layer       attn 49.0M + dense 17.8M + experts 761.3M ≈ 828.1M   (×5)
total ≈ 25.2 B       active ≈ 3.8 B (top-8 of 128 → 47.6M of each layer's 761.3M expert block)
```

The "26B-A4B" name checks out. Residency projections (projections at the stated bpw, embedding
f32 as the engine holds it today):

| plan | arithmetic | ≈ GB |
|---|---|---|
| int8-resident | 24.48 B × 1.125 + 2.95 | **30.5** |
| q4-resident | 24.48 B × 0.625 + 2.95 | **18.3** |
| IQ1_S mix (gate_up_exps native 1.5625 bpw; down_exps q4; attn+dense int8) | 2.97 + 4.76 + 1.85 + 2.95 | **12.5** |

KV is ragged per layer: `25·(2·2048·4) + 5·(2·1024·4)` = **440 KiB/token** f32 → ~3.6 GB at 8k
context. Verdict: **an M2-class (39.5 GiB) target, comfortably even at int8+KV (~34 GB); NOT an
FP5/PinePhone-class target** — even the 1.58-bit mix plus f32 embedding plus KV overruns 8 GB.
An FP5 build would additionally need the embedding held quantized (~0.6 GB at q6_k) and would
still be tight. `PerLayerHeadDim` is also why the ENGINE cannot lay out that ragged KV cache
yet — the CPU reference's `KvCache` holds per-layer widths precisely to demonstrate the shape
the engine layout needs to grow.

## What is PROVEN where (mirrors PLATFORM_VALIDATION.md's honesty)

| claim | where proven | what it does NOT prove |
|---|---|---|
| header/name/census contract (converter ↔ `read_shape` ↔ gate) | sandbox, `--gate-gemmadiff` on the `--selfcheck` fixture | nothing about the real 51 GB drop's tensors |
| whole-model forward composes (per-layer dims, V-from-K, twin FFN, pass scalars, loop) | sandbox CPU, 13 unit tests + gate denoise on FILE weights; composition pin is exact-f32 vs hand-assembled oracles | model QUALITY (fixture weights are random); any GPU behavior |
| IQ1_S file pathway (native pricing to the byte, kill-switch, slice math on file bytes) | sandbox, fixture probe tensor | a real mixed-quant checkpoint end to end |
| SP-BPE tokenizer == HF reference | sandbox: 8 fixture probes + **11/11 on the real 262144-piece tokenizer** via `--emit-tokenizer` + `--gate-tokenizer` | corpus-scale equivalence; exotic merge-rank ties outside 11 strings |
| engine (GPU) forward | **nowhere — eight declared archfeat gaps**, `supported: false` | — |

## Bugs this work found (dated, in the GLM47 tradition)

* **2026-08-18** — `estimate_int8_resident` read `llama.*` keys only, so every non-llama header
  returned None and the app's budget guard **silently skipped itself**; and it priced everything
  as int8, judging an IQ1_S native file **5.76× too big** — a wrong-refusal that is invisible
  because a refusal produces no artifact to inspect. Replaced by the arch-independent
  tensor-table walk; the 5.76× factor is pinned by a test.
* **2026-08-18** — `memplan::iq1s_load_plan` had tests and **zero callers**, and its doc said
  "the native path is not wired", which had been false since the type-19 fast path landed. A doc
  that asserts absence sends the next reader hunting for a gap that closed.
* **2026-08-18** — `chat.rs::replicator_hosts` used `gpu_kernels` outside its feature gate:
  `cargo check -p logscope-egui` (no features) had been broken on main. Found only because both
  feature configs get checked before touching a crate.
* **2026-08-18** — the gate's author guessed `attn_post_norm` where the gemma4 convention is
  `post_attention_norm`; **the gate caught its own author** on the first file load. That is the
  entire argument for making the gate LOAD the file rather than just read the header.
* **2026-08-18** — the selfcheck fixture shipped canvas 8 with the real scheduler's block 32;
  `read_shape` refused it. Root cause one level up: the converter HARDCODED `block_length: 32`
  with a comment naming the file it should have read. Both fixed: scheduler/generation configs
  are read when present, and `canvas % block` is refused at convert time — failing there beats
  failing at load time by the length of a 51 GB write.
* **2026-08-18** — gguf-py's `add_tensor(raw_dtype=…)` takes the data's **BYTE shape**
  `(rows, bytes_per_row)` and derives the logical width itself; handing it the logical shape
  raises `bytes per row not a multiple of type size`. Recorded because the error text does not
  say which shape it wanted.

## What the engine forward still needs (the eight gaps, by lift)

Structural (the layout cannot represent the model at all): **PerLayerHeadDim** (ragged KV +
per-layer q/k/v dims through cache, kernels, and dims buffers) and **AltAttnValueFromKey** for
the affected layers. Correctness-critical but shape-compatible: TwinFfnParallelResidual,
RouterInputNormScale, PerExpertOutputScale, LayerOutputScale/EncDecLayerScale (needs a scalar
multiply on the residual stream per layer per pass). Semantics-blocked: SelfConditioning (no
reference to implement against). Every one has a CPU oracle in `gemmadiff.rs`/`gemmadiffcpu.rs`
to be gated bit-exact against, which is the point of this whole tier: when the M2 work starts,
"done" is a diff against a number, not a vibe.

Pointers: `tools/convert-diffusion-gemma/README.md` (usage, open items),
`src/gemmadiff.rs` (plan + op oracles + scheduler), `src/gemmadiffcpu.rs` (whole-model
reference + loop), `kernel-validate --gate-gemmadiff` / `--gate-tokenizer`.
