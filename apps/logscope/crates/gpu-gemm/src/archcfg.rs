//! archcfg.rs — SCRATCHWORK (2026-06-12) toward multi-architecture support. Pure, no wgpu, testable.
//!
//! The loader hard-refuses any `general.architecture` other than "llama". This module is the
//! groundwork to lift that: a registry that, per architecture, records the GGUF metadata key prefix,
//! the tensor-name quirks, and the *kernel* differences (attention QKV bias, activation, norm variant,
//! logit/attention soft-capping, RoPE style). It does NOT implement the kernels — those need the M2 to
//! validate against a reference — but it makes the differences explicit and gives the loader a single
//! place to ask "what does this arch need, and do we support it yet?" instead of a hardcoded string ==.
//!
//! Roadmap order (per the loader's own note): Qwen2 next (just adds QKV bias on top of llama), then
//! Gemma2 (geglu + dual soft-cap + a norm tweak), then Phi-3 (fused QKV / partial-rotary).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch { Llama, Qwen2, Qwen3, Qwen35, Qwen3Moe, Qwen4, Llama4, Gemma2, Gemma3, DiffusionGemma, Phi3, Glm4Moe, DeepseekV3, Unknown }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Norm { /// x/rms(x) · w
    Rms, /// gemma: x/rms(x) · (1 + w), and norms computed in f32
    RmsGemma }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act { /// silu(gate) · up
    SwiGlu, /// gelu(gate) · up
    GeGlu }

#[derive(Clone, Debug, PartialEq)]
pub struct ArchConfig {
    pub arch: Arch,
    pub prefix: &'static str,         // GGUF metadata key prefix, e.g. "llama" → "llama.embedding_length"
    pub attn_qkv_bias: bool,          // qkv projections have bias tensors (Qwen2)
    pub attn_qk_norm: bool,           // per-head RMSNorm on Q and K before RoPE (Qwen3 / Qwen3.5 / Qwen4 / Llama4)
    pub norm: Norm,
    pub act: Act,
    pub rope_interleaved: bool,       // true = llama-style ADJACENT-pair RoPE (ggml NORM); false = NEOX split-half (qwen/gemma/phi). WIRED: loader sets Gpu.rope_neox = !this
    pub moe: bool,                    // mixture-of-experts: a router (ffn_gate_inp) + N expert FFNs (Qwen4 / Llama4)
    pub moe_shared_expert: bool,      // an always-on shared expert alongside the routed ones (Llama4)
    pub interleaved_nope: bool,       // some attention layers use NO positional encoding (Llama4 iRoPE)
    pub logit_softcap: Option<f32>,   // Gemma2 final-logit soft cap
    pub attn_softcap: Option<f32>,    // Gemma2 attention-logit soft cap
    pub tied_embed_default: bool,     // output usually tied to embedding
    pub supported: bool,              // kernels wired in the engine TODAY (full arch, incl. MoE if any)
    pub dense_supported: bool,        // the NON-MoE forward path works on existing kernels — so a dense
                                      // checkpoint of this arch (expert_count == 0) loads even when `supported`
                                      // is false because the MoE routing kernel isn't wired yet
    pub note: &'static str,           // what's required to support it (or "" when supported)
}

/// Map a `general.architecture` string (None/"" → llama, since some converters omit it) to its config.
/// Is this architecture supported, DERIVED from what archfeat says is missing?
///
/// The `supported` field beside each entry was hand-maintained, and this session produced five bugs
/// where it disagreed with reality — archfeat claiming Gemma support this denied, a flag that stayed
/// false for four commits after the code landed, a note describing a blocker that had been fixed.
/// Every one was the cost of two descriptions of one fact kept in sync by hand.
///
/// llama.cpp has no capability matrix at all: an architecture is in the enum and works, or it is
/// unsupported. That is the shape to copy. Here the matrix is load-bearing — the refusals name which
/// features are missing, which is genuinely more useful than "no" — so instead of removing it,
/// SUPPORTED IS DEFINED AS "archfeat lists no gaps". One source of truth, computed.
///
/// `variant` is the expert count, since Llama-4 Scout and Maverick differ by it.
pub fn is_supported(arch: &str, variant: u32) -> bool {
    // ARCHFEAT VETOES, IT DOES NOT GRANT. Writing this as `gaps.is_empty()` alone failed immediately on
    // two architectures, and both failures were right:
    //
    //   deepseek2 has NO known gaps since _294 implemented the split MLA layout, yet is declared
    //   unsupported because it has never completed a forward pass on a real checkpoint. "No gap I know
    //   of" is not "verified", and a derivation that conflated them would have declared MLA working on
    //   the strength of my own feature table.
    //
    //   gemma3 IS supported, and AttentionScaleHparam is listed as a gap — true only of the 27B, which
    //   the loader already refuses by layer count. A gap that applies to one size cannot veto the
    //   others, and `gaps(arch, 0)` cannot see which size it is being asked about.
    //
    // So a gap is sufficient to refuse and its absence is not sufficient to accept. That asymmetry is
    // what makes this safe to derive at all: the drift it removes is "the flag says yes while a gap is
    // listed", which is the direction that ships broken output.
    for_arch(arch).supported && crate::archfeat::gaps(arch, variant).is_empty()
}

/// What the hand-maintained flag says, kept only so the drift test can compare them.
///
/// Callers should use `is_supported`. This exists to make disagreement a TEST FAILURE rather than a
/// runtime surprise, and can be deleted once every entry has been reconciled.
pub fn declared_supported(arch: &str) -> bool { for_arch(arch).supported }

pub fn for_arch(arch: &str) -> ArchConfig {
    match arch {
        "" | "llama" => ArchConfig {
            arch: Arch::Llama, prefix: "llama", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true, note: "",
        },
        "qwen2" => ArchConfig {
            arch: Arch::Qwen2, prefix: "qwen2", attn_qkv_bias: true, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "Q/K/V bias add (blk.N.attn_{q,k,v}.bias) loaded + applied in Lin::apply; validated by --gate-qwen2 (correctness). Bit-exact-vs-real-Qwen2 still M2-gated.",
        },
        // Qwen3 drops Qwen2's QKV bias and adds per-head QK-Norm: an RMSNorm (blk.N.attn_q_norm.weight /
        // attn_k_norm.weight, each [head_dim]) applied to Q and K before RoPE. Everything else is llama-shape
        // (GQA, SwiGLU, RMSNorm). Reuses rmsnorm_t over [t*heads, head_dim], so no new kernel.
        "qwen3" => ArchConfig {
            arch: Arch::Qwen3, prefix: "qwen3", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "per-head QK-Norm (attn_q_norm/attn_k_norm) applied before RoPE; correctness via --gate-qwen3. Bit-exact-vs-real-Qwen3 M2-gated.",
        },
        // Qwen3.5 (e.g. Ornith-1.0-9B, general.architecture = "qwen3_5"): same forward-pass shape as Qwen3
        // for our purposes (QK-Norm, no QKV bias, GQA, SwiGLU, RMSNorm). Reads qwen3_5.* metadata.
        "qwen3_5" | "qwen35" => ArchConfig {
            arch: Arch::Qwen35, prefix: "qwen3_5", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "Qwen3.5 dense (Ornith): QK-Norm before RoPE, qwen3_5.* metadata; correctness via --gate-qwen3. M2-gated for bit-exactness.",
        },
        // Qwen3-MoE (general.architecture = "qwen3moe"): the Qwen3 QK-Norm attention base + mixture-of-
        // experts — a router (ffn_gate_inp) selecting top-k of N experts (ffn_{gate,up,down}_exps), NO shared
        // expert (unlike Qwen2-MoE). This is Qwen3-30B-A3B / Qwen3-Coder-30B-A3B (128 experts, 8 active).
        // Softmax gating. Same expert shape as a dense Qwen3 FFN, so each selected expert reuses the existing
        // matvec/SwiGLU kernels; the router is a small matvec + top-k (moe::route).
        // Qwen3.5 / Qwen3.6 MoE — general.architecture = "qwen35moe". c4-804 mapped this to the
        // qwen3moe shape on the reasoning that "that is what the family has been", and the operator's
        // first load falsified it: blk.0 of Qwen3.6-35B-A3B carries ssm_a, ssm_alpha, ssm_beta,
        // ssm_conv1d, ssm_dt.bias, ssm_norm and ssm_out alongside attn_qkv and attn_gate. It is the
        // same hybrid GATED DELTANET architecture as Qwen3.8, at a size that looked runnable.
        //
        // A DeltaNet layer reuses the attention projections for q/k/v and adds the recurrent state,
        // so attn_* being present says nothing; the ssm_* tensors are what distinguish the layer
        // types, and gguf::ssm_layer_census counts them. Observed shape: 256 experts, a shared expert
        // with its OWN gate (ffn_gate_inp_shexp), fused attn_qkv [2048x8192], attn_gate [2048x4096].
        "qwen35moe" | "qwen36moe" | "qwen3_5moe" => ArchConfig {
            prefix: "qwen35moe",
            supported: false, dense_supported: false,
            note: "Qwen3.5/3.6 35B-A3B is HYBRID GATED DELTANET, not a larger Qwen3-30B-A3B — \
observed in the operator's own tensor dump, which is why the family guess was wrong. The ssm_* \
tensors per block are a recurrent state this engine has no path for, so those layers have no forward \
pass at all rather than a slow one; only the ~1-in-4 gated-attention blocks are shapes we run. It \
stays unverified end to end because nothing can run it. The MATH now has a CPU oracle: \
src/deltanet.rs transcribes llama.cpp's delta-net-base.cpp autoregressive recurrence (decay -> \
delta correction -> output, conv ring, gated norm) with invariant tests — GPU work checks against \
it. The MoE side is fine: 256 experts, shared expert with its own gate, and Q4_K/MXFP4 both slice \
since c4-784 and c4-805",
            ..for_arch("qwen3moe")
        },
        "qwen3moe" => ArchConfig {
            arch: Arch::Qwen3Moe, prefix: "qwen3moe", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: false, interleaved_nope: false,
            logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "Qwen3-MoE (Qwen3-Coder-30B-A3B confirmed: d=2048, 48 layers, n_heads=32, n_kv_heads=4, head_dim=128 [NOTE head_dim != d/n_heads=64], 128 experts × ff=768, top-8, NO shared expert; ffn_*_exps stacked with the expert dim OUTERMOST, each slice contiguous & 256-divisible; attn_v/ffn_down_exps are mixed Q6_K/Q4_K per layer). Needs: (1) head_dim decoupled from d/n_heads in the attention path [moe::layer_shape], (2) per-expert dequant→requant of the stacked ffn_*_exps in the loader, (3) MoE forward route→top-8→weighted expert sum [moe::route, ready]. Reuses the existing matvec/SwiGLU kernels per expert.",
        },
        // RND1 (general.architecture = "rnd1"): Radical Numerics' masked-DIFFUSION conversion of
        // Qwen3-30B-A3B — identical tensor/metadata shape to qwen3moe (128 experts, top-8, QK-Norm,
        // NO shared expert, NEOX rope), trained to denoise rather than to continue. Loads through the
        // Qwen3Moe machinery unchanged; what differs is the FORWARD the caller runs (denoise_stats:
        // bidirectional attention over the whole block, no KV accumulation) and the mask token, read
        // from tokenizer.ggml.mask_token_id (gguf_mask_token_id in lib). AR-decoding this checkpoint
        // emits fluent-looking noise — it has no causal training; that is the model, not a bug.
        "rnd1" => ArchConfig {
            arch: Arch::Qwen3Moe, prefix: "rnd1", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: false, interleaved_nope: false,
            logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "RND1-30B-A3B: qwen3moe-shaped diffusion model; run the denoise path (denoise-bench), not AR decode. Mask id from tokenizer.ggml.mask_token_id.",
        },
        // Qwen4 (general.architecture = "qwen4"): continues Qwen3.5's per-head QK-Norm base and adds
        // mixture-of-experts — a router (blk.N.ffn_gate_inp.weight) selecting top-k of N expert FFNs
        // (ffn_{gate,up,down}_exps.weight). The DENSE forward (QK-Norm → RoPE → GQA → SwiGLU → RMSNorm) is
        // already wired, so a dense Qwen4 checkpoint (expert_count == 0) loads via the existing path; a MoE
        // checkpoint needs the top-k routing + expert-gather kernel (M2-gated, like the other new kernels).
        "qwen4" => ArchConfig {
            arch: Arch::Qwen4, prefix: "qwen4", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: false, interleaved_nope: false,
            logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: false, dense_supported: true,
            note: "Qwen4 adds mixture-of-experts (ffn_gate_inp router + ffn_{gate,up,down}_exps) on the Qwen3.5 QK-Norm base. The claim that a MoE checkpoint NEEDS a top-k routing + expert-gather kernel is STALE (c4-799): that machinery ships and is exercised daily by qwen3moe and llada2. A qwen3moe fixture rebranded as qwen4 loads and produces BIT-IDENTICAL stats, so the forward path is the one already in use. `supported` stays false only because this project flips it on a real checkpoint, never a synthetic — what is missing is a Qwen4 GGUF to run, not code.",
        },
        // Llama4 (general.architecture = "llama4"): MoE with a shared expert (Scout/Maverick), interleaved
        // NoPE — a subset of attention layers use NO positional encoding (iRoPE, for length generalization) —
        // and QK-Norm. Inherently MoE (no meaningful dense form), so no dense escape; needs the MoE routing
        // kernel + a per-layer RoPE/NoPE switch. Config-complete here so the loader names exactly what's missing.
        "llama4" => ArchConfig {
            arch: Arch::Llama4, prefix: "llama4", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: true, moe_shared_expert: true, interleaved_nope: true,
            logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: true, dense_supported: false,
            note: "Llama4 MoE: 16 routed (top-1, raw-sigmoid gate on the expert INPUT) + 1 shared expert, interleaved NoPE every 4th layer, weightless QK-Norm on RoPE layers, iRoPE attention temperature tuning on NoPE layers (length generalization past 8K). Runs the v1 routed path (v2 GPU-routing covers softmax gating only, so far).",
        },
        "gemma" | "gemma2" => ArchConfig {
            arch: Arch::Gemma2, prefix: "gemma2", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::RmsGemma, act: Act::GeGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: Some(30.0), attn_softcap: Some(50.0), tied_embed_default: true,
            supported: false, dense_supported: false,
            note: "needs geglu activation, (1+w) rms norm, and tanh soft-cap on attn + final logits",
        },
        // Gemma 3: everything Gemma 2 has EXCEPT the soft-caps (gemma3.cpp defaults
        // f_final_logit_softcapping to 0), plus learned per-head Q/K norms applied BEFORE rope — the same
        // shape Qwen3 uses, which is why attn_qk_norm carries it. Sliding window, (1+w) norm, GELU gating
        // and the post-norms are all implemented and gated (--gate-swa, --gate-gemma).
        "gemma3" | "gemma3n" => ArchConfig {
            arch: Arch::Gemma3, prefix: "gemma3", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::RmsGemma, act: Act::GeGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "gemma3: (1+w) rms norm, geglu, post-norms and sliding-window attention",
        },
        // Gemma 4, including the E2B/E4B "effective parameter" variants aimed at phones and 8GB
        // machines. Architecturally Gemma 3's successor — (1+w) RMS norm, GeGLU, per-head QK-Norm
        // before RoPE, NEOX pairing, no soft-caps — so it reuses that path rather than getting a
        // speculative one of its own.
        //
        // supported = false DELIBERATELY, and this is the honest part: nothing here has run a Gemma 4
        // checkpoint. The entry exists so the loader reports "recognised, unverified" instead of
        // "unknown architecture", which are different problems for whoever hits it — one means send a
        // file, the other means write a kernel. It flips to true when a real checkpoint passes the
        // gate, not before.
        "gemma4" | "gemma4e" => ArchConfig {
            arch: Arch::Gemma3, prefix: "gemma4", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::RmsGemma, act: Act::GeGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: true, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: false, dense_supported: false,
            note: "gemma4/E-series: STILL unverified against a real checkpoint, but no longer a guess — \
the vendored llama.cpp reference (src/models/gemma4.cpp) settles the shape this entry previously \
ASSUMED was Gemma 3's. The 26B-A4B is a MoE: dense FFN as a shared expert beside 128 routed \
(top-8, softmax gating, GELU), a TWIN parallel FFN residual with its own norm set \
(pre_ffw_norm_2 / post_ffw_norm_1 / post_ffw_norm_2), V taken FROM THE K PROJECTION on \
full-attention layers (attn_v optional; weightless rms on V), per-layer head_dim \
(key_length_swa vs key_length), per-layer learned output scale, proportional rope via per-layer \
rope_freqs tensors on full layers, and the FINAL-logit soft cap back at 30.0. E2B/E4B add \
per-layer token embeddings on top. archfeat::required(\"gemma4\") names what of this the engine \
lacks; the entry exists so the refusal names the gaps rather than the family",
        },
        // DiffusionGemma (google/diffusiongemma-26B-A4B, general.architecture = "diffusion_gemma"):
        // BLOCK-DIFFUSION text generation on the Gemma 4 26B-A4B MoE stack above. One weight stack,
        // two passes: a causal ENCODE pass that appends finished canvases to the KV cache, and a
        // bidirectional DENOISE pass over the current canvas — each with its OWN learned per-layer
        // output scalar — plus a self-conditioning FFN at the decoder top and the BlockRefinement
        // scheduler (entropy-bound commits, full renoise, 0.8→0.4 anneal, confident-AND-stable
        // early stop). The plan and the scheduler live in `gemmadiff` (pure, tested); the mask
        // token is `<mask>` (tokenizer.ggml.mask_token_id), and diffusion.shift_logits is FALSE —
        // canvas denoising predicts in place, LLaDA-lineage.
        "diffusion_gemma" | "diffusiongemma" => ArchConfig {
            arch: Arch::DiffusionGemma, prefix: "diffusion_gemma", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::RmsGemma, act: Act::GeGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: true, interleaved_nope: false, logit_softcap: Some(30.0), attn_softcap: None, tied_embed_default: true,
            supported: false, dense_supported: false,
            note: "DiffusionGemma 26B-A4B: the Gemma 4 MoE base (twin parallel FFN, V-from-K on full \
layers, per-layer head_dim/kv/rope, per-layer output scale, softcap 30) plus diffusion — an \
encoder-mode scalar set (blk.N.layer_output_scale_enc), a self_cond.* FFN, and block-refinement \
decoding. gemmadiff::read_shape validates the header; archfeat::required(\"diffusion_gemma\") \
names the kernels this engine lacks. Flips to supported on a real checkpoint through a gate, \
never on the fixture",
        },
        "phi3" => ArchConfig {
            arch: Arch::Phi3, prefix: "phi3", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: false, dense_supported: false,
            note: "fused qkv tensor + partial-rotary RoPE; needs a split + partial-dim rope kernel",
        },
        // GLM-4-MoE (general.architecture = "glm4moe"). Two sub-forms exist; this registry entry is the
        // STANDARD-attention one (GQA + per-head QK-Norm + qkv bias, like Qwen3, plus shared-expert MoE).
        // The MLA sub-form is detected by tensor presence in the loader and refused there. NEOX rope.
        "glm4moe" => ArchConfig {
            arch: Arch::Glm4Moe, prefix: "glm4moe", attn_qkv_bias: true, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: true, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: true, dense_supported: true,
            note: "",
        },
        // DeepSeek-V2/V3 (+ GLM-MoE with MLA): Multi-head Latent Attention. Detected here with the right
        // GLM-5.2 (`glm-dsa`). SEPARATE from DeepSeek despite sharing MLA, because `prefix` selects the
        // METADATA KEY namespace and a glm-dsa file spells its keys `glm-dsa.attention.*`. Folding it
        // into the DeepSeek entry meant looking up `deepseek2.attention.kv_lora_rank` in a file that has
        // `glm-dsa.attention.kv_lora_rank` — so the loader reported "no kv_lora_rank in its metadata"
        // about a checkpoint that states it plainly. A wrong reason is worse than no reason: it sends
        // the reader to the checkpoint when the fault is ours.
        // LLaDA2 (general.architecture = "llada2"): Ant's masked-DIFFUSION conversion of the
        // Ling-2.0 / bailingmoe2 family — LLaDA2.0-mini is 16B total, ~1.4B active. Spec source:
        // the llama.cpp llada2 PR (converter tensor list + llm_build_llada2 graph), which is
        // Glm4Moe-shaped end to end: fused attn_qkv (loader's fused fallback), per-head QK-Norm
        // BEFORE RoPE, NEOX rope, sigmoid routing normalized over the selected k, shared expert
        // added to the routed output, leading_dense_block_count dense lead (per-layer detection
        // handles it), and trailing nextn/MTP blocks (generic nextn_predict_layers exclusion).
        // KNOWN GAP, inherited from the GLM/DSV3 support here: blk.N.exp_probs_b (the no-aux
        // selection bias) is not fetched — expert SELECTION runs unbiased until that lands.
        // Mechanics and throughput are unaffected; routing fidelity is approximate. The forward
        // is caller-side: run the denoise path (denoise-bench); AR-decoding this checkpoint
        // emits fluent noise by design. Mask id from tokenizer.ggml.mask_token_id;
        // diffusion.shift_logits is expected FALSE (LLaDA lineage predicts in place).
        "llada2" => ArchConfig {
            arch: Arch::Glm4Moe, prefix: "llada2", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: true, interleaved_nope: false,
            logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: true, dense_supported: true,
            note: "LLaDA2.0 (Ling-2.0 diffusion): run the denoise path, not AR decode. exp_probs_b selection bias wired (reference order: bias steers selection, weights stay unbiased).",
        },
        "glm-dsa" | "glm_dsa" => ArchConfig {
            arch: Arch::DeepseekV3, prefix: "glm-dsa", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: true, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: false, dense_supported: false,
            note: "GLM-5.2 (744B-A40B, 78 layers, MLA + DeepSeek Sparse Attention). LOADS end to end on \
the synthetic (gen_glmdsa.py: MLA + MoE + IndexShare + split 3-D kv_b — load + finite decode via \
kernel-validate --probe); `supported` stays false until a REAL checkpoint produces coherent text. \
NOT blocked on the indexer for correctness: dense attention is EXACT at or below index_topk (2048). \
llama.cpp master (Aug 2026) now SHIPS DSA — src/models/glm-dsa.cpp + llama-kv-cache-dsa.h, lightning \
indexer with per-layer indexer_types and TENSOR_NOT_REQUIRED indexer tensors — the reference to \
transcribe when long-context efficiency matters. IndexShare (~1 layer in 4 carries indexer tensors) \
is a non-issue for our loader (unread tensors are not fetched). Remaining real blocker for the full \
model: IQ1_S is 201.8 GiB against 192 GB — the IQ3 reader or the MoE-residency lever. \
expert_weights_scale 2.5 and expert_weights_norm are wired. Order of work in docs/glm-5.2-arch.md",
        },
        "deepseek2" | "deepseek3" | "deepseekv3" => ArchConfig {
            arch: Arch::DeepseekV3, prefix: "deepseek2", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: true, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: false, dense_supported: false,
            note: "MLA kernels AND loading are wired. Present: the compressed-latent cache, per-layer \
routing (decode_step_full sends any layer with `w.mla` to block_decode_mla), the decode block \
(--gate-mla-block), BOTH kv_b layouts — legacy fused attn_kv_b and the modern split attn_k_b + \
attn_v_b, which GGUF stores 3-D but which are the same bytes as llama.cpp's 2-D declaration \
(PR #12801) — and per-BLOCK loading in the resident loader: MLA blocks get placeholder q/k/v and \
the low-rank set, incomplete sets fail naming the set. --gate-mla-load proves load + 3-token \
decode on a q-compressed synthetic (this work landed in b911a471, was LOST in a merge, and is \
re-landed with the gate as its regression net). `supported` stays false because numerics on a \
REAL checkpoint (GLM-4.7-Flash q4 on real hardware) remain unverified — coherent output there is \
what flips this flag, never a synthetic",
        },
        // Dream / DiffuCoder: a Qwen2.5 backbone trained for masked diffusion rather than AR
        // continuation — same QKV-bias attention, RMSNorm and SwiGLU as "qwen2", only the
        // metadata prefix and the generation loop differ. Confirmed on a real checkpoint
        // (DiffuCoder-7B-Instruct q4_k_m): n_embd=3584, 28 layers, 28/4 heads, n_ff=18944.
        //
        // The mask id comes from tokenizer.ggml.mask_token_id and is NOT a constant — it is
        // 151666 on that checkpoint. gguf_mask_token_id() already reads it; this entry only
        // lets the loader resolve the hparam keys, which it could not do while "dream" fell
        // through to the unknown arm and got an empty prefix.
        "dream" => ArchConfig {
            arch: Arch::Qwen2, prefix: "dream", attn_qkv_bias: true, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "Same tensor shapes and kernels as qwen2; only the metadata prefix differs. Runs the diffusion denoise loop, not AR continuation. Verified by a forward pass on DiffuCoder-7B-Instruct — which is the bar this file sets for `supported`.",
        },
        // LLaDA: a Llama backbone trained for masked diffusion. No QKV bias, ADJACENT-pair
        // rope, identical shape to "llama". Mask id from the GGUF, as above.
        "llada" => ArchConfig {
            arch: Arch::Llama, prefix: "llada", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "Same tensor shapes and kernels as llama; only the metadata prefix differs. NOT llada-moe — that prefix carries expert_* keys nobody has inspected, and assuming this entry covers it is how a MoE model loads with its routing silently ignored.",
        },
        _ => ArchConfig {
            arch: Arch::Unknown, prefix: "", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: false, dense_supported: false, note: "unknown architecture — use the llama.cpp backend",
        },
    }
}

/// The metadata key names the loader should read for this arch (generalizing the hardcoded "llama.*").
#[derive(Clone, Debug, PartialEq)]
pub struct DimKeys {
    pub embedding_length: String,
    pub feed_forward_length: String,
    pub block_count: String,
    pub head_count: String,
    pub head_count_kv: String,
    pub rope_freq_base: String,
    pub rms_eps: String,
    pub expert_count: String,        // MoE: total routed experts ({pfx}.expert_count); 0/absent = dense
    pub expert_used_count: String,   // MoE: top-k experts activated per token
    pub expert_shared_count: String, // MoE: always-on shared experts (Llama4)
}
pub fn dim_keys(cfg: &ArchConfig) -> DimKeys {
    let p = cfg.prefix;
    DimKeys {
        embedding_length: format!("{p}.embedding_length"),
        feed_forward_length: format!("{p}.feed_forward_length"),
        block_count: format!("{p}.block_count"),
        head_count: format!("{p}.attention.head_count"),
        head_count_kv: format!("{p}.attention.head_count_kv"),
        rope_freq_base: format!("{p}.rope.freq_base"),
        rms_eps: format!("{p}.attention.layer_norm_rms_epsilon"),
        expert_count: format!("{p}.expert_count"),
        expert_used_count: format!("{p}.expert_used_count"),
        expert_shared_count: format!("{p}.expert_shared_count"),
    }
}

/// Per-block MoE tensor names (router + expert FFN stacks, + a shared expert for Llama4) — present only when
/// `moe`. Empty for dense architectures. These are the tensors the routing/expert-gather kernel will bind.
pub fn moe_tensors(cfg: &ArchConfig, layer: usize) -> Vec<String> {
    if !cfg.moe { return Vec::new(); }
    let mut v = vec![
        format!("blk.{layer}.ffn_gate_inp.weight"),   // router logits: [n_expert, d]
        format!("blk.{layer}.ffn_gate_exps.weight"),  // stacked expert gate: [n_expert, ffn, d]
        format!("blk.{layer}.ffn_up_exps.weight"),    // stacked expert up:   [n_expert, ffn, d]
        format!("blk.{layer}.ffn_down_exps.weight"),  // stacked expert down: [n_expert, d, ffn]
    ];
    if cfg.moe_shared_expert {
        v.push(format!("blk.{layer}.ffn_gate_shexp.weight"));
        v.push(format!("blk.{layer}.ffn_up_shexp.weight"));
        v.push(format!("blk.{layer}.ffn_down_shexp.weight"));
    }
    v
}

/// The router gate normalization for this arch: softmax for the Qwen family, sigmoid for Llama4-style gating.
pub fn route_norm(cfg: &ArchConfig) -> crate::moe::RouteNorm {
    match cfg.arch {
        Arch::Llama4 => crate::moe::RouteNorm::Sigmoid,
        _ => crate::moe::RouteNorm::Softmax,
    }
}

/// Build the MoE routing config from an arch + the checkpoint's expert counts (from `dim_keys`). `n_used`
/// falls back to a sensible default when the metadata omits it.
/// `scale` and `norm` come from the checkpoint's `{arch}.expert_weights_scale` and
/// `{arch}.expert_weights_norm` — the key names llama.cpp uses, arch-prefixed. Passed in rather than
/// read here so this stays free of the Gguf, and taken as Options so "the metadata omits it" is
/// distinguishable from "the metadata says 1.0".
///
/// The prefix matters more than it looks: reading these under the WRONG arch prefix returns None for a
/// model that set them, which is exactly how glm-dsa's kv_lora_rank was misread as absent.
pub fn moe_config(cfg: &ArchConfig, n_routed: u32, n_used: u32, n_shared: u32,
                  scale: Option<f32>, norm: Option<bool>) -> crate::moe::MoeConfig {
    crate::moe::MoeConfig {
        n_routed,
        n_used: if n_used == 0 { 8.min(n_routed.max(1)) } else { n_used },
        n_shared: if cfg.moe_shared_expert { n_shared.max(1) } else { n_shared },
        norm: route_norm(cfg),
        // Llama4: raw-sigmoid top-1 gate applied to the expert's INPUT (weight-before-FFN);
        // everything else: normalized weights combined on the output.
        // Metadata wins; the arch default is the fallback. llama.cpp reads this from the checkpoint
        // rather than deriving it, and a model that sets expert_weights_norm=false without being
        // Llama-4 would otherwise be normalized against its own config.
        // METADATA WINS, EXCEPT WHERE THE ARCHITECTURE FORBIDS IT.
        //
        // Making this metadata-driven was right for Qwen and DeepSeek, and I introduced a risk with
        // it: Llama-4's gate is not a normalization TOGGLE, it is a different forward pass. The raw
        // sigmoid multiplies the expert's INPUT (weight_before), and a normalized weight applied there
        // is not the same computation scaled — it is wrong. A checkpoint that happens to carry
        // expert_weights_norm=true would have silently switched Llama-4 into a shape it does not have.
        //
        // So the arch VETOES rather than merely defaulting, which is archfeat's rule arriving here.
        // ARCHITECTURAL WHERE THE ARCHITECTURE DECIDES IT, metadata only where it does not.
        //
        // Checked against llama.cpp's per-model builders, and it hardcodes norm_w as a LITERAL for
        // both of these — not from hparams:
        //
        //     models/llama4.cpp:228     LLM_FFN_SILU, false,   <- Llama-4
        //     models/qwen3moe.cpp:144   LLM_FFN_SILU, true,    <- Qwen3-MoE
        //
        // So my _370 change was wrong for BOTH, not just Llama-4. A Qwen checkpoint carrying
        // expert_weights_norm=false would have switched off a normalization the architecture requires,
        // and the output would have been plausible rather than broken.
        //
        // The scale is different and IS read from metadata in both files
        // (hparams.expert_weights_scale), which is why routed_scale stays metadata-driven.
        normalize: match cfg.arch {
            Arch::Llama4 => false,
            Arch::Qwen3Moe => true,
            _ => norm.unwrap_or(true),
        },
        // GLM-5.2 sets routed_scaling_factor (2.5); everything else leaves it inert at 1.0. Applied
        // AFTER normalization — see the ordering note on the field.
        routed_scale: scale.unwrap_or(1.0),
        weight_before: matches!(cfg.arch, Arch::Llama4),
        // Group-limited routing arrives from METADATA at the loader (expert_group_count kvs) or the
        // llada2 architecture default — this constructor leaves it off and the loader overrides.
        n_group: 0, topk_group: 0 }
}

/// Per-block bias tensor names — present only when `attn_qkv_bias` (Qwen2). Empty otherwise.
pub fn qkv_bias_tensors(cfg: &ArchConfig, layer: usize) -> Vec<String> {
    if cfg.attn_qkv_bias {
        vec![format!("blk.{layer}.attn_q.bias"), format!("blk.{layer}.attn_k.bias"), format!("blk.{layer}.attn_v.bias")]
    } else { Vec::new() }
}

/// The per-head QK-Norm weight tensors for a layer (Qwen3/3.5), or empty for architectures without it.
pub fn qk_norm_tensors(cfg: &ArchConfig, layer: usize) -> Vec<String> {
    if cfg.attn_qk_norm {
        vec![format!("blk.{layer}.attn_q_norm.weight"), format!("blk.{layer}.attn_k_norm.weight")]
    } else { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unverified_arch_can_never_verify_ITSELF_without_an_opt_in() {
        // The deadlock this documents. `supported` flips only once an architecture has completed a
        // forward pass on a real checkpoint — the right rule, and the reason deepseek2 stayed gated
        // with no known gaps. But attempting the run IS what produces that evidence, and the loader
        // refused the attempt, so the flag could never change.
        //
        // glm-dsa is precisely there: two of its three gaps implemented, the third a no-op below 2048
        // context, MLA layout fixed — and unrunnable. LOGSCOPE_TRY_UNSUPPORTED_ARCH is the escape, and
        // it claims nothing: it lets someone find out.
        assert!(!is_supported("glm-dsa", 0), "still unverified, correctly");
    }

    #[test]
    fn diffusion_gemma_is_recognised_gapped_and_gemma_shaped() {
        let c = for_arch("diffusion_gemma");
        assert_eq!(c.arch, Arch::DiffusionGemma);
        assert_eq!(c.prefix, "diffusion_gemma", "metadata reads under its own namespace");
        assert!(c.moe && c.moe_shared_expert, "26B-A4B: 128 routed + the dense FFN as shared expert");
        assert!(c.attn_qk_norm && c.norm == Norm::RmsGemma && c.act == Act::GeGlu,
                "Gemma lineage: learned QK-norm, (1+w) rms, GeGLU");
        assert_eq!(c.logit_softcap, Some(30.0), "the final-logit cap is back in this generation");
        assert!(!c.supported && !c.dense_supported,
                "recognised-but-refused until a real checkpoint passes a gate");
        assert!(!is_supported("diffusion_gemma", 128), "gaps veto regardless of the flag");
        assert_eq!(for_arch("diffusiongemma").arch, Arch::DiffusionGemma, "converter spelling variant");
        // and recognition means the refusal can NAME things: at least one gap must exist to name
        assert!(!crate::archfeat::gaps("diffusion_gemma", 128).is_empty());
    }

    #[test]
    fn gemma4_now_carries_what_the_reference_settled() {
        // The entry previously said "assumed Gemma 3 shape" — the vendored gemma4.cpp shows the
        // 26B-A4B is a shared-expert MoE. The flags must match the reference, and the note must
        // no longer present the assumption as the whole story.
        let c = for_arch("gemma4");
        assert!(c.moe && c.moe_shared_expert, "26B-A4B is a MoE with the dense FFN as shared expert");
        assert!(c.logit_softcap.is_none(),
                "the DEFAULT stays metadata-driven — only diffusion_gemma's shipped config pins 30.0");
        assert!(!c.note.contains("assumed Gemma 3 shape"), "the guess is retired: {}", c.note);
        assert!(c.note.contains("gemma4.cpp"), "and the note says where the shape now comes from");
        assert!(!declared_supported("glm-dsa"), "and the declaration is what gates it, not the gaps");
        // The gap list is now down to one, which is what makes the attempt worth offering.
        assert_eq!(crate::archfeat::gaps("glm-dsa", 0).len(), 1,
            "only the sparse indexer remains: {:?}", crate::archfeat::gaps("glm-dsa", 0));
    }

    #[test]
    fn the_nope_interval_decides_which_layers_ROTATE() {
        // Scout's gibberish has survived every GPU-side fix on both platforms, and this is the kind of
        // thing that would produce it: nope_step was hardcoded to 4, so every fourth layer skips
        // rotation. That is Scout's documented pattern and an ASSUMPTION about every other Llama-4
        // checkpoint — Maverick, and anything shipped later.
        //
        // A wrong interval applies RoPE to a layer that should have none and vice versa. Every tensor
        // is present, every shape matches, the model loads, and the text is garbled. Nothing in a
        // shape check can see it, which is why it outlived them.
        //
        // The rule, as the loader applies it: layer i is NoPE when (i + 1) % step == 0.
        let nope_at = |i: usize, step: usize| step > 0 && (i + 1) % step == 0;
        for i in 0..12 {
            assert_eq!(nope_at(i, 4), matches!(i, 3 | 7 | 11), "step 4, layer {i}");
        }
        // A different interval selects a different set — which is the whole reason it must be read.
        assert!(nope_at(1, 2) && !nope_at(1, 4), "step 2 and step 4 disagree at layer 1");
        // step 0 means no NoPE layers at all, which is every non-Llama-4 architecture.
        for i in 0..12 { assert!(!nope_at(i, 0)); }
    }

    #[test]
    fn llama_is_supported_today() {
        let c = for_arch("llama");
        assert!(c.supported && c.note.is_empty());
        assert_eq!(for_arch(""), c, "missing arch string falls back to llama");    }

    #[test]
    fn deepseek_and_glmdsa_are_mla_but_glm4moe_is_standard() {
        // Corrected against real checkpoints: the MLA family (DeepSeek-V3, GLM-5.2 'glm-dsa') is detected
        // but unwired; GLM4-MoE ('glm4moe') uses STANDARD GQA attention + shared-expert MoE and IS supported.
        for name in ["deepseek2", "deepseek3", "deepseekv3", "glm-dsa", "glm_dsa"] {
            let c = for_arch(name);
            assert_eq!(c.arch, Arch::DeepseekV3, "{name} should map to the MLA arch");
            assert!(c.moe && c.moe_shared_expert, "DeepSeek/GLM-DSA has routed + shared experts");
            assert!(!c.supported, "MLA loads and decodes on synthetics, but no REAL checkpoint has \
run → stays declared-unsupported until one does (LOGSCOPE_TRY_UNSUPPORTED_ARCH is the escape)");
            assert!(c.note.contains("MLA") || c.note.contains("Latent"), "note must explain the MLA gap");
        }
        // glm4moe: standard attention (QK-norm + bias) + shared-expert MoE — supported.
        let g = for_arch("glm4moe");
        assert_eq!(g.arch, Arch::Glm4Moe);
        assert!(g.supported && g.moe && g.moe_shared_expert && g.attn_qk_norm && g.attn_qkv_bias,
            "glm4moe is a supported standard-attention shared-expert MoE with QK-norm + bias");
    }

    #[test]
    fn qwen2_is_llama_plus_bias() {
        let c = for_arch("qwen2");
        assert_eq!(c.arch, Arch::Qwen2);
        assert!(c.attn_qkv_bias, "qwen2's distinguishing feature is qkv bias");
        assert_eq!(c.act, Act::SwiGlu);
        assert_eq!(c.norm, Norm::Rms);
        assert!(c.supported && c.note.contains("bias"), "now wired (bias add) + validated by --gate-qwen2");
        assert_eq!(qkv_bias_tensors(&c, 7), vec!["blk.7.attn_q.bias", "blk.7.attn_k.bias", "blk.7.attn_v.bias"]);
    }

    #[test]
    fn qwen3_drops_bias_adds_qk_norm() {
        let c = for_arch("qwen3");
        assert_eq!(c.arch, Arch::Qwen3);
        assert!(!c.attn_qkv_bias, "qwen3 removed qwen2's qkv bias");
        assert!(c.attn_qk_norm, "qwen3's distinguishing feature is per-head QK-Norm");
        assert!(c.supported && c.act == Act::SwiGlu && c.norm == Norm::Rms);
        assert_eq!(qk_norm_tensors(&c, 5), vec!["blk.5.attn_q_norm.weight", "blk.5.attn_k_norm.weight"]);
        assert!(qkv_bias_tensors(&c, 5).is_empty());
    }

    #[test]
    fn qwen4_is_qwen35_plus_moe_with_a_dense_escape() {
        let c = for_arch("qwen4");
        assert_eq!(c.arch, Arch::Qwen4);
        assert!(c.attn_qk_norm, "qwen4 keeps the QK-Norm base");
        assert!(c.moe && !c.moe_shared_expert && !c.interleaved_nope);
        assert!(!c.supported, "MoE routing kernel isn't wired yet");
        assert!(c.dense_supported, "a dense qwen4 (expert_count 0) rides the existing QK-Norm path");
        // router + stacked expert FFN tensors are named for the eventual kernel
        let mt = moe_tensors(&c, 3);
        assert!(mt.contains(&"blk.3.ffn_gate_inp.weight".to_string()));
        assert!(mt.contains(&"blk.3.ffn_down_exps.weight".to_string()));
        assert!(!mt.iter().any(|t| t.contains("shexp")), "qwen4 has no shared expert");
        assert_eq!(dim_keys(&c).expert_count, "qwen4.expert_count");
    }

    #[test]
    fn llama4_is_moe_with_shared_expert_and_interleaved_nope() {
        let c = for_arch("llama4");
        assert_eq!(c.arch, Arch::Llama4);
        assert!(c.moe && c.moe_shared_expert, "llama4 has routed + one shared expert");
        assert!(c.interleaved_nope, "llama4's iRoPE: some layers use no positional encoding");
        assert!(c.attn_qk_norm);
        assert!(c.supported, "llama4 runs: shared expert + NoPE switch + sigmoid weight-before gating landed");
        assert!(!c.dense_supported, "inherently MoE + NoPE — no dense forward to fall back to");
        assert!(!c.tied_embed_default);
        // shared-expert tensors present alongside the routed stacks
        let mt = moe_tensors(&c, 0);
        assert!(mt.iter().any(|t| t == "blk.0.ffn_gate_shexp.weight"));
        assert!(mt.iter().any(|t| t == "blk.0.ffn_gate_exps.weight"));
        assert!(c.note.contains("NoPE") && c.note.contains("shared expert"));
    }

    #[test]
    fn rope_pairing_by_family() {
        // llama rotates adjacent pairs (ggml NORM); qwen/gemma/phi rotate split halves (NEOX).
        // Wrong pairing = real words, scrambled meaning (the dense-Qwen salad, root-caused on M2 Ultra).
        assert!(for_arch("llama").rope_interleaved);
        for a in ["qwen2", "qwen3", "qwen3moe", "gemma2", "gemma3", "phi3"] {
            assert!(!for_arch(a).rope_interleaved, "{a} must be NEOX");
        }
    }

    #[test]
    fn qwen3moe_is_qwen3_qk_norm_plus_moe_no_shared() {
        let c = for_arch("qwen3moe");
        assert_eq!(c.arch, Arch::Qwen3Moe);
        assert!(c.attn_qk_norm && !c.attn_qkv_bias, "Qwen3 QK-Norm base");
        assert!(c.moe && !c.moe_shared_expert, "MoE with NO shared expert (unlike Qwen2-MoE)");
        assert_eq!(route_norm(&c), crate::moe::RouteNorm::Softmax);
        // Qwen3-30B-A3B: 128 experts, 8 active, no shared
        let mc = moe_config(&c, 128, 8, 0, None, None);
        assert_eq!(mc.n_routed, 128);
        assert_eq!(mc.n_used, 8);
        assert_eq!(mc.n_shared, 0);
        assert!(c.note.contains("30B-A3B"));
        // router + expert tensors named, no shared-expert tensors
        let mt = moe_tensors(&c, 0);
        assert!(mt.iter().any(|t| t == "blk.0.ffn_gate_inp.weight"));
        assert!(!mt.iter().any(|t| t.contains("shexp")));
    }

    #[test]
    fn moe_config_maps_arch_to_router() {
        // Qwen4: softmax gating, no shared expert
        let q = for_arch("qwen4");
        let qc = moe_config(&q, 128, 8, 0, None, None);
        assert_eq!(qc.norm, crate::moe::RouteNorm::Softmax);
        assert_eq!(qc.n_used, 8);
        assert_eq!(qc.n_shared, 0);
        // Llama4: sigmoid gating, at least one shared expert
        let l = for_arch("llama4");
        let lc = moe_config(&l, 128, 1, 1, None, None);
        assert_eq!(lc.norm, crate::moe::RouteNorm::Sigmoid);
        assert!(lc.n_shared >= 1, "llama4 forces a shared expert");
        // n_used defaults when metadata omits it
        assert_eq!(moe_config(&q, 64, 0, 0, None, None).n_used, 8);
    }

    #[test]
    fn dense_archs_emit_no_moe_tensors() {
        for a in ["llama", "qwen2", "qwen3", "qwen3_5"] {
            assert!(moe_tensors(&for_arch(a), 0).is_empty(), "{a} is dense — no expert tensors");
            assert!(!for_arch(a).moe);
        }
    }

    #[test]
    fn qwen3_5_ornith_reads_its_own_prefix() {
        let c = for_arch("qwen3_5");
        assert_eq!(c.arch, Arch::Qwen35);
        assert!(c.attn_qk_norm && !c.attn_qkv_bias && c.supported);
        assert_eq!(dim_keys(&c).embedding_length, "qwen3_5.embedding_length");
        // the "qwen35" spelling maps to the same config
        assert_eq!(for_arch("qwen35").arch, Arch::Qwen35);
    }

    #[test]
    fn a_note_must_not_claim_something_is_UNWIRED_that_archfeat_says_is_implemented() {
        // Third instance of the same drift this session (gemma4's missing entry, glm-dsa's prefix, now
        // this). GLM-4.7-Flash printed BOTH of these in one load:
        //     "MLA attention — compressed-latent KV cache (validated); per-layer routing active"
        //     "load failed: ... the MLA forward isn't wired yet"
        // The engine and its own error message disagreed, because the note was written before MLA
        // landed and nothing forced it to be revisited.
        for a in ["deepseek2", "deepseek3", "glm-dsa", "glm4moe"] {
            let n = for_arch(a).note.to_ascii_lowercase();
            let claims_unwired = n.contains("isn't wired") || n.contains("is not wired")
                || n.contains("not implemented");
            let mla_done = crate::archfeat::Feature::Mla.implemented();
            assert!(!(claims_unwired && mla_done),
                "{a}'s note says MLA is unwired but archfeat reports it implemented: {n}");
        }
        // The deepseek note has now said FOUR different things across four commits, and the sequence
        // is the point: "MLA isn't wired" (stale), then "the blocker is 3-D tensor layout" (true at
        // the time), then "both layouts are read but LOADING is absent" (true again after a merge
        // dropped b911a471's loader half), and now that loading is re-landed with --gate-mla-load as
        // its net. Each revision followed a real change, and a note that lagged behind sent the
        // reader to inspect their checkpoint for our bug.
        let n = for_arch("deepseek2").note;
        assert!(n.contains("attn_k_b") && n.contains("attn_kv_b"), "both layouts named: {n}");
        // and it must still refuse, because reading the weights is not the same as having run them
        assert!(!for_arch("deepseek2").supported);
        assert!(n.contains("remain unverified"), "the remaining gap has to stay stated: {n}");
    }

    #[test]
    fn the_registry_prefix_can_DISAGREE_with_the_file_and_the_file_wins() {
        // Qwen3.5 ships files whose keys are `qwen35.*` while the registry says `qwen3_5`. The loader
        // sniffed the real prefix, PRINTED that it was using it, and then never stored it — so every
        // later read through acfg.prefix returned None on that file. Rope base, norm epsilon,
        // expert_weights_scale: each defaulted silently, and a wrong rope base produces exactly the
        // near-random output reported ("doctrine%," in answer to "hi there").
        //
        // NOTICING AND NOT ACTING IS WORSE THAN NOT NOTICING: the log claimed the file's prefix was in
        // use, and it was not, so the one message that could have pointed here said the opposite.
        let a = for_arch("qwen35");
        let b = for_arch("qwen3_5");
        assert_eq!(a.arch, b.arch, "both spellings are the same architecture");
        // The registry's prefix is one spelling; a file using the other must override it, which is now
        // done at the sniff site rather than left in a local.
        assert_eq!(a.prefix, "qwen3_5", "registry default");
    }

    #[test]
    fn checkpoint_metadata_OVERRIDES_the_arch_default() {
        // llama.cpp reads expert_weights_norm from the checkpoint rather than deriving it from the
        // architecture. A model that sets it false without being Llama-4 would otherwise be normalized
        // against its own config — and the output would be plausible, not broken.
        // Rewritten twice as the reference check narrowed it. Qwen and Llama-4 are now BOTH
        // architectural — llama.cpp hardcodes norm_w for each — so this exercises an arch that has
        // not been checked against a reference, where the checkpoint is still the better source.
        let ds = for_arch("deepseek2");
        assert!(moe_config(&ds, 128, 8, 0, None, None).normalize, "default when absent");
        assert!(!moe_config(&ds, 128, 8, 0, None, Some(false)).normalize, "metadata wins");
        // I asserted the other direction here in _370 — that metadata could turn Llama-4's gate ON.
        // That is the case now vetoed: its raw sigmoid multiplies the expert's INPUT, so a normalized
        // weight there is a different forward pass, not the same one scaled. The test asserted the
        // behaviour and the behaviour was the bug.
        assert!(!moe_config(&for_arch("llama4"), 16, 1, 1, None, Some(true)).normalize,
            "the architecture vetoes; see LLAMA4_ignores_expert_weights_norm");
    }

    #[test]
    fn LLAMA4_ignores_expert_weights_norm_because_its_gate_is_ARCHITECTURAL() {
        // A risk I introduced when making this metadata-driven in _370. Llama-4's raw sigmoid
        // multiplies the expert's INPUT, so a normalized weight applied there is not the same
        // computation scaled — it is a different forward pass. A checkpoint carrying
        // expert_weights_norm=true would have silently switched it into a shape it does not have.
        let l4 = for_arch("llama4");
        assert!(!moe_config(&l4, 16, 1, 1, None, Some(true)).normalize,
            "metadata cannot turn Llama-4's raw-sigmoid gate into a normalized one");
        assert!(!moe_config(&l4, 16, 1, 1, None, None).normalize);
        assert!(moe_config(&l4, 16, 1, 1, None, None).weight_before,
            "and the weight still goes to the expert's input, which is why");
    }

    #[test]
    fn QWEN3MOE_also_ignores_the_metadata_and_llamacpp_is_why() {
        // The check found this and it widened the last fix. llama.cpp hardcodes norm_w as a LITERAL
        // in both per-model builders:
        //
        //     models/llama4.cpp:228     LLM_FFN_SILU, false,
        //     models/qwen3moe.cpp:144   LLM_FFN_SILU, true,
        //
        // Neither reads hparams.expert_weights_norm. So a Qwen checkpoint carrying
        // expert_weights_norm=false would have switched off a normalization the architecture
        // requires — the same class of silent breakage as the Llama-4 case, in the other direction.
        let q = for_arch("qwen3moe");
        assert!(moe_config(&q, 128, 8, 0, None, Some(false)).normalize,
            "the architecture requires it; metadata cannot switch it off");
        assert!(moe_config(&q, 128, 8, 0, None, None).normalize);
    }

    #[test]
    fn the_SCALE_is_still_metadata_driven_for_both() {
        // The scale IS read from hparams in both files, which is why it stays metadata-driven — the
        // two knobs are not the same kind of thing and treating them alike was the original mistake.
        for a in ["llama4", "qwen3moe"] {
            let c = for_arch(a);
            assert_eq!(moe_config(&c, 16, 1, 1, Some(2.5), None).routed_scale, 2.5, "{a}");
        }
    }

    #[test]
    fn an_architecture_we_have_not_checked_still_takes_the_metadata() {
        // The hardcoding is only justified where a reference implementation was read. Elsewhere the
        // checkpoint remains the better source.
        let d = for_arch("deepseek2");
        assert!(!moe_config(&d, 128, 8, 0, None, Some(false)).normalize);
    }

    #[test]
    fn an_absent_scale_is_INERT_rather_than_zero() {
        // None means the metadata omitted it, which must leave every weight untouched. Defaulting to
        // 0.0 would silence every routed expert and look like a broken router.
        assert_eq!(moe_config(&for_arch("qwen3moe"), 128, 8, 0, None, None).routed_scale, 1.0);
        assert_eq!(moe_config(&for_arch("qwen3moe"), 128, 8, 0, Some(2.5), None).routed_scale, 2.5);
    }

    #[test]
    fn the_declared_flag_and_the_derived_answer_do_not_disagree() {
        // The bug class this closes. Five times this session a hand-maintained flag said one thing and
        // archfeat said another, and each was found by a user hitting it rather than by a test. Now a
        // disagreement fails here.
        //
        // Listed explicitly rather than iterating every arch, because the list IS the reconciliation:
        // an entry appears here once someone has checked that both descriptions agree, and adding one
        // is a deliberate act.
        // A gap must be able to VETO a declared-supported arch — that is the direction that ships
        // broken output, and the only one worth failing a build over.
        for (arch, variant) in [("llama", 0u32), ("llama4", 16), ("llama4", 128),
                                ("qwen2", 0), ("qwen3moe", 0)] {
            if !crate::archfeat::gaps(arch, variant).is_empty() {
                assert!(!is_supported(arch, variant),
                    "{arch} (variant {variant}) has gaps {:?} yet is_supported says yes",
                    crate::archfeat::gaps(arch, variant));
            }
        }
        // and the reverse must NOT hold: an arch with no gaps can still be unsupported, because
        // "no gap I know of" is not "verified on a real checkpoint".
        assert!(crate::archfeat::gaps("deepseek2", 0).is_empty(), "the split layout landed in _294");
        assert!(!is_supported("deepseek2", 0), "but MLA has never run end to end");
    }

    #[test]
    fn an_arch_with_gaps_is_not_supported_however_the_flag_reads() {
        // The derivation, stated as a property: gaps and support are the same question. deepseek2 has
        // an unimplemented feature and must therefore be unsupported, whatever anyone typed.
        // gemma3 is the case that proves the veto has to be variant-aware rather than blanket:
        // AttentionScaleHparam is a gap only on the 27B, which the loader refuses by layer count.
        assert!(declared_supported("gemma3"), "the 12B and below work");
        // llama4 has no gaps and is declared supported, so it passes both halves
        assert!(crate::archfeat::gaps("llama4", 16).is_empty(), "{:?}", crate::archfeat::gaps("llama4", 16));
        assert!(is_supported("llama4", 16));
    }

    #[test]
    fn a_shared_config_entry_must_not_share_a_metadata_prefix() {
        // `prefix` selects the metadata KEY namespace, and GGUF spells those keys with the file's own
        // architecture string. GLM-5.2 was folded into the DeepSeek entry, so the loader looked up
        // `deepseek2.attention.kv_lora_rank` in a file that carries `glm-dsa.attention.kv_lora_rank` and
        // reported the metadata as MISSING. It was not missing; we asked the wrong question.
        assert_eq!(for_arch("glm-dsa").prefix, "glm-dsa");
        assert_eq!(for_arch("glm_dsa").prefix, "glm-dsa", "the underscore spelling resolves the same");
        assert_eq!(for_arch("deepseek3").prefix, "deepseek2");
        // they still SHARE the forward-pass shape — only the namespace differs
        assert_eq!(for_arch("glm-dsa").arch, for_arch("deepseek3").arch);
        // The note must name what ACTUALLY blocks a load, and that changed once the upstream picture
        // was checked (c4-785): llama.cpp does not implement DSA either and runs GLM-5.2 dense, so
        // the indexer is a long-context cost rather than a prerequisite. What does block a load is
        // IndexShare — only ~1 layer in 4 carries indexer tensors, which is what breaks stock
        // llama.cpp on converted GGUFs — and the 201.8 GiB against 192 GB fit. This guard used to
        // demand the word INDEXER, which would now keep a stale diagnosis alive.
        let note = for_arch("glm-dsa").note;
        assert!(note.contains("IndexShare"), "the note must name the loader trap: {note}");
        // Case-insensitive: the note emphasises DENSE in caps, and a guard that fails on
        // capitalisation tests the prose rather than the claim.
        assert!(note.to_ascii_lowercase().contains("dense"), "and that mainline runs this model dense: {note}");
        assert!(note.contains("201.8 GiB") || note.contains("IQ3"), "and the fit problem: {note}");
    }

    #[test]
    fn gemma3_is_enabled_and_differs_from_gemma2_where_llamacpp_says() {
        let c = for_arch("gemma3");
        assert!(c.supported && c.dense_supported, "gemma3's requirements are implemented and gated");
        // shared with gemma2
        assert_eq!(c.norm, Norm::RmsGemma, "x/rms(x)·(1+w)");
        assert_eq!(c.act, Act::GeGlu);
        assert!(!c.rope_interleaved, "NEOX split-half, like qwen/gemma/phi");
        // NOT shared: gemma3.cpp defaults the soft-caps off, and adds learned Q/K norms before rope
        assert!(c.logit_softcap.is_none() && c.attn_softcap.is_none(), "gemma3 has no soft-caps");
        assert!(c.attn_qk_norm, "gemma3 adds learned per-head Q/K norms before rope");
        let g2 = for_arch("gemma2");
        assert!(g2.logit_softcap.is_some() && !g2.attn_qk_norm, "gemma2 is the other way round");
        assert!(!g2.supported, "gemma2 still needs soft-capping");
    }

    #[test]
    fn gemma2_flags_its_quirks() {
        let c = for_arch("gemma2");
        assert_eq!(c.act, Act::GeGlu);
        assert_eq!(c.norm, Norm::RmsGemma);
        assert_eq!(c.logit_softcap, Some(30.0));
        assert_eq!(c.attn_softcap, Some(50.0));
        assert!(!c.supported);
    }

    #[test]
    fn dim_keys_use_the_arch_prefix() {
        let k = dim_keys(&for_arch("qwen2"));
        assert_eq!(k.embedding_length, "qwen2.embedding_length");
        assert_eq!(k.head_count_kv, "qwen2.attention.head_count_kv");
        let lk = dim_keys(&for_arch("llama"));
        assert_eq!(lk.embedding_length, "llama.embedding_length");
    }

    #[test]
    fn unknown_arch_is_unsupported_with_guidance() {
        let c = for_arch("mamba");
        assert_eq!(c.arch, Arch::Unknown);
        assert!(!c.supported && c.note.contains("llama.cpp"));
        assert!(qkv_bias_tensors(&c, 0).is_empty());
    }
    #[test]
    fn the_shared_expert_count_comes_from_the_FILE_when_it_says() {
        // The loader passed a literal 0 and it worked for Scout only because moe_config rescues an
        // arch declaring moe_shared_expert with .max(1). Correct by accident — an architecture with
        // TWO shared experts loses one silently, and a missing always-on expert degrades output
        // without breaking a shape.
        let l4 = for_arch("llama4");
        // The rescue still applies when the file says nothing, so Scout is unchanged.
        assert_eq!(moe_config(&l4, 16, 1, 0, None, None).n_shared, 1, "flag rescues a silent file");
        // But a file that declares more must get more.
        assert_eq!(moe_config(&l4, 16, 1, 2, None, None).n_shared, 2, "the file outranks the flag");
        // And an arch with no shared expert keeps none when the file says none.
        let q = for_arch("qwen3moe");
        assert_eq!(moe_config(&q, 128, 8, 0, None, None).n_shared, 0);
    }
}

#[cfg(test)]
mod qwen4_shares_the_qwen3moe_path {
    /// The qwen4 registry note claimed a MoE checkpoint needed routing and expert-gather kernels
    /// this engine did not have. It has them — qwen3moe and llada2 both run on them — and a
    /// qwen3moe fixture rebranded as qwen4 produces bit-identical stats, which is the strongest
    /// statement a synthetic can make: not "it works" but "it takes the same path".
    ///
    /// Kept as a note rather than a flag flip, because `supported` is earned on a real checkpoint.
    /// The point is that the blocker is a download, not an implementation — the same correction
    /// glm-dsa needed in c4-785 and IQ3_XXS in c4-786. Registry gap claims go stale silently when
    /// the gap is closed somewhere else.
    #[test]
    fn the_note_says_what_is_actually_missing() {
        let note = super::for_arch("qwen4").note;
        assert!(note.contains("STALE"), "the superseded claim must be marked: {note}");
        assert!(note.to_ascii_lowercase().contains("real checkpoint"),
            "and the note must say what would flip it: {note}");
        assert!(!super::for_arch("qwen4").supported,
            "still false — a rebranded synthetic is not evidence a real Qwen4 runs");
    }
}

#[cfg(test)]
mod note_claims {
    //! Notes that assert ENGINE STATE, paired with the check that expires them.
    //!
    //! Borrowed from the shape of a Claude "skill" file, whose description carries not just a
    //! trigger but a NEGATIVE one — "do NOT use this skill if the content is already in context".
    //! The condition under which the claim stops holding travels with the claim.
    //!
    //! This registry has no such condition on anything, and it shows. In roughly fifteen bundles
    //! five notes were found asserting things that had stopped being true: group-limited routing
    //! described as "engine support pending" after c4-750 implemented it on both CPU and GPU;
    //! extreme-quant.md claiming IQ2/IQ3 could not be READ when dequantize had dispatched IQ3_XXS
    //! all along, which caused glm52-fit.md to write off the one quant that fits GLM-5.2 in 192 GB;
    //! the sparse-attention gap called STRUCTURAL when llama.cpp runs the same model dense; and this
    //! one, "MLA is wired", which was true of the kernels and false of the loader.
    //!
    //! Every one was found by accident while doing something else. A stale note is worse than no
    //! note: it is a confident answer that sends the reader somewhere the code is not.
    //!
    //! So: if a note claims the engine cannot do something, a test here must FAIL when it can. That
    //! is the only force that reliably updates prose.
    use super::*;

    /// The `glm-dsa` and `deepseek2` notes said MLA loading was absent — and for a stretch they
    /// were right for the WRONG reason. b911a471 implemented per-block loading; a later merge took
    /// the other side of both files (`git log -S "gate-mla-load"` on main finds nothing even though
    /// the commit is an ancestor); and the structural probe that used to live here kept passing
    /// through all of it, because it grepped for symbol names (`load_mla_layer`, `MlaLayerWeights`)
    /// the implementation never used. A pin that greps for a GUESS about a spelling expires
    /// silently; this one greps for the code that exists today, and its failure message says what
    /// to do when that stops being true.
    ///
    /// Inverted from `mla_loading_is_still_absent`: the per-block routing is present, and the notes
    /// claim loading works rather than that it is absent. --gate-mla-load is the behavioural check
    /// (load + 3-token decode on a q-compressed synthetic); this is the prose-vs-code check.
    #[test]
    fn mla_loading_exists_and_the_notes_say_so() {
        let src = include_str!("lib.rs");
        assert!(src.contains("let block_is_mla"),
            "the per-block MLA sniff is gone from the resident loader. If it was renamed, update \
this pin; if a merge dropped it, that is the SECOND loss of this exact work (b911a471 was the \
first) — re-land it and run --gate-mla-load");
        for arch in ["deepseek2", "glm-dsa"] {
            let n = for_arch(arch).note;
            assert!(!n.contains("LOADING IS NOT") && !n.contains("loading is absent"),
                "{arch}'s note claims MLA loading is absent while the loader ships it — a note \
that is wrong about the engine is worse than no note at all");
            assert!(n.contains("gate-mla-load") || n.contains("gen_glmdsa"),
                "{arch}'s note should point at the gate/fixture that proves the load path");
        }
    }

    /// Every dense-attention block fn refuses an MLA layer instead of computing over it.
    ///
    /// The companion to `mla_loading_is_still_absent`, and the reason it is here rather than beside
    /// the blocks: that test says loading is absent, which is what makes THIS one cheap to satisfy
    /// today. The moment loading lands, `BlockWeights.mla` becomes reachable, and `w.wq`/`w.wk`/
    /// `w.wv` — not `Option`, so an MLA layer must carry something in them — become readable by
    /// eight block fns of which only `block_decode_mla` is correct for that layer. The output would
    /// be a correctly-shaped tensor of nonsense, not an error.
    ///
    /// Checked structurally, by the same reasoning as the loading test: a behavioural check needs a
    /// GPU and an MLA checkpoint, and neither exists in the sandbox where this is most likely to be
    /// broken. A grep over the source needs neither and fails on the ninth block fn someone adds
    /// without the guard — which is the case that actually happens.
    #[test]
    fn every_dense_block_refuses_an_mla_layer() {
        let src = include_str!("lib.rs");
        let mut unguarded = Vec::new();
        for (i, line) in src.lines().enumerate() {
            let t = line.trim_start();
            // Signatures only: `fn block_decode…(&self,` at method indentation.
            let Some(rest) = t.strip_prefix("fn ") else { continue };
            if !rest.starts_with("block_decode") || !rest.contains("(&self,") { continue }
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if name == "block_decode_mla" { continue }
            // The guard must be inside the body, not merely somewhere in the file. A signature can
            // span several lines, so scan a window past it rather than only the next line.
            let body: String = src.lines().skip(i).take(14).collect::<Vec<_>>().join("\n");
            let want = format!("refuse_mla_on_dense_block(w, \"{name}\")");
            if !body.contains(&want) { unguarded.push(name); }
        }
        assert!(unguarded.is_empty(),
            "these dense-attention block fns would compute over an MLA layer's absent q/k/v weights \
and return a plausible-looking tensor: {unguarded:?}. Add \
`Self::refuse_mla_on_dense_block(w, \"<fn name>\");` as the first statement, or route the layer to \
block_decode_mla.");
    }

    /// The same expiry, extended to the two design notes that state engine state in prose.
    ///
    /// The registry's notes have this module. `DEEPSEEK_SUPPORT.md` and `BIGGER_MODELS.md` had
    /// nothing, and both went stale in the way this module exists to prevent: each still described
    /// the loader as llama-only, and `BIGGER_MODELS.md` quoted the `general.architecture != "llama"`
    /// refusal by name. That refusal has been an `archcfg` lookup for some time — qwen2, qwen3,
    /// qwen3.5, qwen3moe, gemma3, llama4, llada2, dream and ornith all load — so a reader following
    /// either doc's headline conclusion would plan work that is already done.
    ///
    /// That is not a hypothetical reader. Both docs name Qwen2 as the highest-value next unlock, and
    /// it is the first thing anyone surveying this tree would pick up.
    ///
    /// Checked against `for_arch` rather than against a copy of the claim, so the test tracks the
    /// registry instead of restating it: whichever arch flips next, the docs' phrasing has to keep up.
    #[test]
    fn the_design_notes_do_not_still_call_the_loader_llama_only() {
        let docs = [
            ("DEEPSEEK_SUPPORT.md", include_str!("../../../DEEPSEEK_SUPPORT.md")),
            ("BIGGER_MODELS.md", include_str!("../../../BIGGER_MODELS.md")),
        ];
        // Non-llama architectures the registry says load today. If any of these is true, prose
        // asserting the loader is llama-only is false.
        let also_supported: Vec<&str> = ["qwen2", "qwen3", "qwen3moe", "gemma3", "llama4", "dream"]
            .into_iter().filter(|a| for_arch(a).supported).collect();
        if also_supported.is_empty() { return; } // llama-only really is true; nothing to expire.

        for (name, body) in docs {
            let low = body.to_ascii_lowercase();
            // The exact phrasings that were wrong, ASCII and the en-dash spelling both docs use.
            for claim in ["llama-architecture only", "llama\u{2011}architecture only",
                          "llama-family only", "llama\u{2011}family only",
                          "hard-refuses any `general.architecture != \"llama\"`"] {
                assert!(!low.contains(&claim.to_ascii_lowercase()),
                    "{name} still says {claim:?}, but the registry reports these non-llama \
architectures as supported: {also_supported:?}. Correct the doc — a design note that is wrong \
about the engine sends its reader to build something that exists.");
            }
            // The Qwen2 row specifically: both docs called it the next unlock.
            if for_arch("qwen2").supported {
                assert!(!low.contains("needs qwen2 arch"),
                    "{name} still lists targets as blocked on \"needs Qwen2 arch\", but qwen2 is \
supported: {}", for_arch("qwen2").note);
            }
        }
    }

    /// Every `.rs` beside lib.rs is reachable from some crate root.
    ///
    /// `specloop.rs` sat in `src/` for a long time with no `mod specloop`
    /// anywhere. Not "no caller" — NOT COMPILED. Its six tests had never run,
    /// and nothing in the build could notice. Two documents disagreed about
    /// whether it existed: `dupscan.rs` recorded the audit decision to delete
    /// it and described that as done, while `ASAHI_GPU.md` listed it as the
    /// speculative control loop and told the reader to wire it up. Both were
    /// written in good faith against a file the compiler had never seen.
    ///
    /// An orphan is worse than dead code. Dead code is compiled, type-checked
    /// and tested, so it stays honest; an undeclared file rots silently and
    /// still reads like the real implementation to anyone grepping.
    ///
    /// Roots are lib.rs AND the `[[bin]]` targets — a module declared only in
    /// `main.rs` is compiled, just not into the library.
    #[test]
    fn no_source_file_is_orphaned_from_the_module_tree() {
        // KNOWN ORPHANS, each with the decision that keeps it here. The point
        // of an allowlist rather than a silent skip: a NEW orphan fails
        // immediately, and each existing one has to carry a reason.
        //
        // llada2 and quant_mix were here and are gone — declared once it was
        // checked that neither is a superseded duplicate (llada2 collides with
        // nothing in this crate; quant_mix's only shared names are generic ones
        // in unrelated modules) and that both are std-only with no imports at
        // all. Their 28 tests now run. The entries were removed because THIS
        // TEST demanded it: the expiry check below failed the moment they were
        // declared and named them.
        const KNOWN: &[(&str, &str)] = &[
            ("kernels",
             "the 2026-07-26 shader extraction, PARTIALLY adopted — lib.rs include_str!s one of \
the 90 files directly and carries inline r#\"...\"# consts for 82 more, so kernels.rs itself is the \
unreachable part. The files and the inline consts now agree (see \
every_shadowed_shader_matches_the_inline_source_that_runs), so completing the extraction is a \
mechanical swap. Not done here: it changes which source 13 shaders compile from, on hardware this \
sandbox cannot run."),
        ];

        let roots: Vec<&str> = vec![
            include_str!("lib.rs"),
            include_str!("main.rs"),
            include_str!("serve.rs"),
        ];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut orphans = Vec::new();

        for entry in std::fs::read_dir(&dir).expect("read src/") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if matches!(stem.as_str(), "lib" | "main" | "serve") {
                continue;
            }
            let declared = roots.iter().any(|r| {
                r.contains(&format!("mod {stem};")) || r.contains(&format!("mod {stem} "))
            });
            if !declared && !KNOWN.iter().any(|(k, _)| *k == stem) {
                orphans.push(stem);
            }
        }

        orphans.sort();
        assert!(
            orphans.is_empty(),
            "these files are in src/ but no `mod` declaration in lib.rs, main.rs or serve.rs \
reaches them, so they are not compiled and their tests never run: {orphans:?}. Declare them, \
delete them, or add them to KNOWN above with the reason — an undeclared file still reads like \
the real implementation to whoever greps for it next."
        );

        // And the allowlist must not outlive its entries: an orphan that got
        // wired or deleted should be struck from the list, not left asserting
        // something untrue. Same expiry discipline as the rest of this module.
        for (name, why) in KNOWN {
            let exists = dir.join(format!("{name}.rs")).exists();
            let declared = roots.iter().any(|r| {
                r.contains(&format!("mod {name};")) || r.contains(&format!("mod {name} "))
            });
            assert!(
                exists && !declared,
                "KNOWN lists `{name}` as an orphan, but it is now \
{} — remove the entry. Its recorded reason was: {why}",
                if !exists { "deleted" } else { "declared" }
            );
        }
    }

    /// The extracted `.wgsl` files must match the inline shaders that run.
    ///
    /// `src/kernels/` holds 90 shader files and `kernels.rs` maps each to a
    /// `const` via `include_str!`. `kernels.rs` has no `mod` declaration, so
    /// that mapping is not compiled — but the picture is NOT simply "all dead",
    /// which is what makes the directory dangerous:
    ///
    ///   1 LIVE     `lib.rs` `include_str!`s `swiglu_matvec_q8_0_add.wgsl`
    ///              directly. That file IS the shader the GPU runs.
    ///   82 SHADOWED `lib.rs` carries an inline `r#"..."#` const of the same
    ///              name. The inline one wins; the file is decoration.
    ///   7 ORPHANED  reachable only through the undeclared `kernels.rs`.
    ///
    /// So one sibling in that directory is genuinely load-bearing, which makes
    /// the other 82 read as load-bearing too — they are syntax-highlighted,
    /// greppable, and sit exactly where a shader belongs. Thirteen had silently
    /// diverged from the source that executes, `moe_topk.wgsl` by more than
    /// half its length. Editing one of those does nothing at all.
    ///
    /// Rewriting them from the inline consts costs nothing (nothing compiles
    /// them) and leaves the half-finished 2026-07-26 extraction in a state
    /// where completing it is a mechanical swap rather than a 13-way
    /// reconciliation. This test keeps them honest in the meantime.
    ///
    /// Deliberately does NOT check the orphaned seven: they have no inline
    /// counterpart to compare against, and inventing one would be asserting a
    /// relationship that does not exist.
    #[test]
    fn every_shadowed_shader_matches_the_inline_source_that_runs() {
        let lib = include_str!("lib.rs");
        let krs = include_str!("kernels.rs");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/kernels");

        // file name -> const name, from kernels.rs's include_str! table.
        let mut const_of: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for line in krs.lines() {
            let (Some(c0), Some(f0)) = (line.find("pub const "), line.find("include_str!(\"kernels/"))
                else { continue };
            let cname = line[c0 + 10..].split(':').next().unwrap_or("").trim();
            let rest = &line[f0 + 22..];
            let Some(fname) = rest.split('"').next() else { continue };
            if !cname.is_empty() && !fname.is_empty() { const_of.insert(fname, cname); }
        }
        assert!(const_of.len() > 50, "parsed only {} entries from kernels.rs", const_of.len());

        let mut drifted = Vec::new();
        let mut checked = 0usize;
        for entry in std::fs::read_dir(&dir).expect("src/kernels") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("wgsl") { continue }
            let fname = path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_string();

            // LIVE files are the source of truth; nothing to compare them to.
            if lib.contains(&format!("include_str!(\"kernels/{fname}\")")) { continue }
            let Some(cname) = const_of.get(fname.as_str()) else { continue }; // orphaned

            let needle = format!("const {cname}: &str = r#\"");
            let Some(a) = lib.find(&needle) else { continue };
            let body = &lib[a + needle.len()..];
            let Some(b) = body.find("\"#;") else { continue };

            checked += 1;
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
            if on_disk.trim() != body[..b].trim() { drifted.push(fname); }
        }

        assert!(checked > 50, "only compared {checked} shaders; the parse regressed");
        drifted.sort();
        assert!(
            drifted.is_empty(),
            "these extracted shaders no longer match the inline const lib.rs actually runs: \
{drifted:?}\nEditing a file in src/kernels/ has no effect while `mod kernels` is undeclared — the \
inline r#\"...\"# in lib.rs is what compiles. Change lib.rs, then re-sync the file."
        );
    }

/// Native IQ1_S stays OPT-IN while it has no prefill path.
    ///
    /// `linear_iq1_s_t` asserts `m == 1`, and `Lin::apply` sends `LinKind::Iq1`
    /// to it at any `m`. There is no conversion anywhere — the assertion's own
    /// message used to claim "batched prefill converts instead", and grepping
    /// for that path finds nothing. So a checkpoint loaded natively panics on
    /// its first multi-token forward, which is `prewarm`, during load:
    ///
    /// ```text
    /// assertion `left == right` failed: IQ1_S matvec is decode-only
    ///   left: 2   right: 1
    ///   Gpu::ffn_apply -> Gpu::prewarm -> Engine::load
    /// ```
    ///
    /// Reported on Qwen3-14B-UD-IQ1_S from a fleet run.
    ///
    /// The gate is now `LOGSCOPE_IQ1_NATIVE=1` — present to opt IN, rather than
    /// `LOGSCOPE_NO_IQ1_NATIVE` to opt out. This pins the polarity, because
    /// flipping it back is a one-word edit that turns every IQ1_S load into a
    /// panic again, and no test on this machine would notice: there is no
    /// IQ1_S fixture here, so the runtime path is unreachable and only the
    /// source can be checked.
    ///
    /// Delete this test when a batched IQ1_S path exists — and it should be
    /// deleted then, not weakened.
    #[test]
    fn native_iq1_s_is_opt_in_while_it_has_no_prefill_path() {
        let lib = include_str!("lib.rs");

        // The decode-only assertion must still be there. If it is gone, either
        // a batched path landed (good — remove this test) or the guard was
        // dropped (bad).
        assert!(
            lib.contains("IQ1_S matvec is decode-only"),
            "the m == 1 assertion is gone from linear_iq1_s_t — if a batched IQ1_S path now exists, delete this test; if the guard was merely removed, a batched forward will read garbage instead of panicking, which is worse"
        );

        // And the loader must require the flag to be PRESENT.
        assert!(
            lib.contains(r#"std::env::var("LOGSCOPE_IQ1_NATIVE").is_ok()"#),
            "the native IQ1_S loader path is no longer opt-in. While no prefill path exists, taking it by default means every IQ1_S checkpoint panics inside prewarm during load, before generating a token."
        );
    }

/// The immediate-data (ex push-constant) capability is REACHABLE.
    ///
    /// lib.rs's llama.cpp comparison note recorded this as an open question:
    /// "wgpu 29 does not expose PUSH_CONSTANTS or max_push_constant_size under
    /// the names this crate would need". Those names are gone because wgpu
    /// renamed the feature to IMMEDIATES, not because it is missing.
    ///
    /// This compiles only if the API exists, which is the whole assertion —
    /// `Limits::max_immediate_size` is the field, and its own doc gives Vulkan
    /// 128-256 bytes against our 16-byte dims payload.
    ///
    /// Worth pinning because the note sat as "unverified" while the answer was
    /// one grep away, and a rename is exactly the kind of thing that would put
    /// it back there.
    #[test]
    fn wgpu_exposes_immediate_data_the_dims_buffers_could_use() {
        let l = wgpu::Limits::default();
        // The field exists. Default is 0 — a non-zero size must be requested at
        // device creation with Features::IMMEDIATES, which is the work, not the
        // blocker.
        let _: u32 = l.max_immediate_size;
        assert_eq!(wgpu::IMMEDIATE_DATA_ALIGNMENT, 4,
            "dims are u32s; if the alignment ever exceeds 4 the payload needs padding");
        assert!(crate::DIMS_PUSH_BYTES <= 128,
            "the dims payload is {} bytes; Vulkan's documented floor for immediates is 128, and \
above it the substitution stops being free",
            crate::DIMS_PUSH_BYTES);
    }

    /// The mirror: notes must not describe as PENDING anything the engine already honours. Group
    /// routing was reported pending for 50 bundles after it landed.
    #[test]
    fn no_note_calls_a_shipped_feature_pending() {
        for arch in ["llada2", "glm-dsa", "deepseek3", "qwen3moe"] {
            let note = for_arch(arch).note.to_ascii_lowercase();
            if note.contains("group") && note.contains("pending") {
                panic!("{arch}: group-limited routing has been honored on CPU and GPU since c4-750, \
but this note still calls it pending");
            }
        }
    }

    /// A note claiming something is UNVERIFIED must not also claim it is validated by a gate — the
    /// two together mean the reader cannot tell which is current.
    #[test]
    fn a_note_does_not_claim_both_unverified_and_gated() {
        for arch in ["llada2", "glm-dsa", "deepseek3", "qwen2", "qwen3", "gemma4"] {
            let n = for_arch(arch).note.to_ascii_lowercase();
            let unverified = n.contains("unverified");
            let validated = n.contains("validated by") || n.contains("correctness via");
            assert!(!(unverified && validated),
                "{arch}: the note says both UNVERIFIED and validated-by-a-gate. One of them is out \
of date and a reader has no way to tell which");
        }
    }
}
