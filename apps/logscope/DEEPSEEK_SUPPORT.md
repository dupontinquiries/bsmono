# Supporting DeepSeek‑style models — design analysis

*This is a design note, not an implementation. It maps what "DeepSeek support" actually requires against the engine as it stands (llama‑architecture only) and lays out a realistic path.*

## Verdict up front

"DeepSeek" names **two architecturally unrelated things**, and conflating them is the main trap:

1. **The R1 *distills*** (`DeepSeek-R1-Distill-Llama-8B`, `…-Qwen-1.5B/7B/14B/32B`). These are **ordinary dense Llama/Qwen models** fine‑tuned on R1's reasoning traces. They have **no MLA and no MoE**. The Llama ones should run on the engine **today**; the Qwen ones need a **small** architecture addition.
2. **Native DeepSeek‑V2 / V3 / R1** (the real models). These use **MLA** (latent‑compressed attention) and **MoE** (routed experts) and are **very large**. Supporting them natively is a **major engine lift** and, at V3 scale (671 B params), out of range for a single 32 GB UMA GPU regardless.

So the high‑leverage move is **not** to chase native V3. It's to (a) confirm the Llama distills already work, and (b) add **Qwen2** architecture — a modest change that unlocks the entire DeepSeek‑R1‑Distill‑Qwen line *and* Qwen2.5 generally. Native MLA+MoE is a separate, much larger project worth scoping only if a small MoE model (e.g. DeepSeek‑V2‑Lite) becomes the actual goal.

## What each target needs vs. what the engine has

The engine implements the Llama path: interleaved‑NORM RoPE, GQA, dense SwiGLU FFN, RMSNorm, and a full per‑KV‑head K/V cache. It reads GGUF metadata under the **`llama.*`** prefix.

| Target | Attention | FFN | Extra vs. Llama | Runs on engine now? |
|---|---|---|---|---|
| R1‑Distill‑**Llama**‑8B | GQA (same) | dense SwiGLU (same) | none — it *is* Llama | **Yes** (try it) |
| R1‑Distill‑**Qwen**‑1.5B…32B | GQA | dense SwiGLU | **Q/K/V bias terms**, different RoPE θ, `qwen2.*` metadata keys | No — needs Qwen2 arch |
| Qwen2.5 / Qwen2 base | GQA | dense SwiGLU | same as above | No — needs Qwen2 arch |
| DeepSeek‑V2‑Lite (16 B, ~2.4 B active) | **MLA** | **MoE** (+ shared experts) | new attention + routing | No — large lift |
| DeepSeek‑V3 / R1 (671 B) | **MLA** | **MoE** (256 experts) | new attention + routing | No — and far too large for 32 GB |

> The current "qwen2.5 fails to load → *missing embedding_length*" is exactly this: the loader looks up `llama.embedding_length`, but a Qwen GGUF stores `qwen2.embedding_length`. The model isn't broken; the engine just isn't reading its metadata namespace.

## Tractability tiers

| Tier | Work | Unlocks | Risk |
|---|---|---|---|
| **0 — already works** | none; just point the loader at a Llama‑arch distill GGUF | R1‑Distill‑Llama‑8B | none |
| **1 — small lift** | add Qwen2 arch: read `qwen2.*` keys, add Q/K/V **bias**, set RoPE θ from metadata | all R1‑Distill‑Qwen sizes + Qwen2/2.5 | low — same attention/FFN math, gateable bit‑exact |
| **2 — large lift** | implement **MLA** (latent KV cache + decompress kernels) and **MoE** (router + per‑expert FFN + much larger weight residency) | DeepSeek‑V2‑Lite class | high — new kernels, new cache layout, memory pressure |
| **3 — not viable here** | native V3/R1 at full scale | — | 671 B params won't fit 32 GB at any quant |

## Detail

### Tier 0 — the Llama distill already fits the engine
`DeepSeek-R1-Distill-Llama-8B` is Llama‑3.1 8B with R1 reasoning SFT. Architecture, RoPE style, GQA, and FFN are all the engine's existing path; an 8B at Q4_K_M is ~4.7 GB, comfortable on the M2. The only thing to confirm is that the GGUF's arch metadata reads as `llama`. This is a "try the existing `--probe`/`--gen` on the file" task, not a code change. **Bonus:** R1 distills emit `<think>…</think>` reasoning, which the egui chat already segments and renders as a collapsible "thinking" block — so the reasoning UX is in place for free.

### Tier 1 — Qwen2 is the high‑leverage addition
Qwen2/Qwen2.5 (and therefore every R1‑Distill‑Qwen) is **Llama with three deltas**:
- **Q/K/V projections carry a bias** (Llama's are bias‑free). The linear layer needs an optional bias add on the attention projections.
- **RoPE θ (rope_freq_base)** differs and must be read from metadata rather than assumed.
- **Metadata namespace is `qwen2.*`**, plus the tokenizer is Qwen's BPE.

Everything else — interleaved RoPE application, GQA grouping, SwiGLU, RMSNorm, the int8 weight path, the KV cache, the whole coordinator/trie stack — is unchanged. Concretely this is: generalize the metadata read to pick the arch prefix from `general.architecture`; thread an optional bias through the attention `Lin`; parameterize RoPE θ. It is **bit‑exact gateable** the same way every other change has been (synthetic Qwen‑shaped model vs. a CPU reference), and it roughly **doubles** the model catalog the engine can run. If any single thing is worth doing for "DeepSeek support," it's this — because the realistic DeepSeek models people actually run locally are the Qwen distills.

### Tier 2 — MLA and MoE are real engine projects, not tweaks

**MLA (Multi‑head Latent Attention).** Instead of caching full per‑head K and V, DeepSeek compresses them into a low‑rank latent vector (≈512‑dim) that is cached, then decompressed per head at attention time, with a small decoupled‑RoPE key dimension carried alongside. This is the *point* of MLA — the KV cache shrinks dramatically — but it means:
- new weights (down/up projections for Q, K, V; the decoupled RoPE key path),
- a **different cache layout** (store the latent, not K/V), and
- a **different attention kernel** (decompress → attend).

It does not reuse the engine's GQA K/V cache; it replaces it. This is its own correctness‑gated subproject.

**MoE FFN.** The dense SwiGLU block is replaced by a **router** (a linear gate → top‑k softmax over E experts) plus per‑token routing to the selected expert FFNs, often with one or more always‑on **shared experts**. Implications:
- a router kernel and a gather/scatter of tokens to experts,
- **all experts resident in memory** (this dominates footprint — V3 has 256 experts; even V2‑Lite has 64), and
- load‑balancing details (the engine can ignore the training‑time aux loss but must match inference routing exactly).

On a 32 GB UMA M2, only a *small* MoE (V2‑Lite‑scale, ~16 B total at int8 ≈ 16 GB) is even arguable, and only after both MLA and MoE exist and are gated. This is the right thing to scope **only if** a specific small MoE model is the actual target.

### Tier 3 — full V3/R1 is a hardware non‑starter here
DeepSeek‑V3/R1 is 671 B parameters (~37 B active). No quantization brings the *resident* weight set into 32 GB. This isn't an engine limitation to fix; it's a model‑size reality. The engine's whole design — dependency‑minimal, single‑GPU, int8‑resident small models — points the other way.

## Recommendation

1. **Now:** run the existing `--probe`/`--gen` against `DeepSeek-R1-Distill-Llama-8B` to confirm Tier 0. The `<think>` rendering already in chat makes this a good showcase with zero engine work.
2. **Next, if you want broad DeepSeek coverage:** do **Tier 1 (Qwen2 arch)**. Small, bit‑exact‑gateable, and it unlocks the R1‑Distill‑Qwen family plus Qwen2.5 — the DeepSeek‑flavored models that actually run on this class of hardware.
3. **Only on a concrete need:** scope **Tier 2 (MLA + MoE)** against one specific small MoE model, as its own correctness‑gated effort. Treat native V3/R1 (Tier 3) as out of scope for a 32 GB single‑GPU engine.

The honest one‑liner: *the engine can already run "DeepSeek" if you mean the Llama distill; it's one modest arch addition away from the Qwen distills; and it's a real project away from anything that uses DeepSeek's actual MLA+MoE architecture — which at full scale won't fit this hardware anyway.*
