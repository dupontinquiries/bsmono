# Fitting GLM-5.2 in 192 GB

The declared minimum is ~223 GB against 192 GB of RAM. Two levers, and the obvious one is probably the
weaker.

## Lever 1: quantization — works, and costs the model

```
current (q4-ish)   223.0 GB   does not fit
IQ3_XXS            151.6 GB   fits
IQ2_XS             114.5 GB   fits
IQ2_XXS            102.1 GB   fits
```

IQ3_XXS clears 192 GB with room for KV. But per `extreme-quant.md`, we cannot READ IQ2/IQ3 today, the
loss concentrates in exactly the capabilities a coding assistant needs, and a 200B-class model at IQ2 is
not "the same model, smaller" — it is a weaker model with the same parameter count.

## Lever 2: residency — it is an MoE, and it never touches most of itself

This is the one worth trying first, and it is nearly free.

An MoE routes each token to a handful of experts. At DeepSeek-class ratios (8 of 256) the active FFN
fraction is **3%**; even at Llama-4's 1-of-16 it is 6%. The remaining 94–97% of the file is not read for
that token, and possibly not for the next hundred.

`mmap` already exploits this: the file stays on disk, pages fault in as experts are routed, and the
kernel's page cache holds whatever is hot. **The constraint becomes the working set rather than the
file.** A 223 GB model with a 20 GB hot set behaves like a 20 GB model that occasionally stalls.

Three things make this concrete here:

* The loader already maps rather than reads — `retain_map` keeps the mapping and
  `release_stale_map_residency` hands back page cache for models that are no longer active.
* The CPU path reads Q4_K **straight from the mapped bytes** with no dequantization pass and no host
  copy, so an expert that is paged in is immediately usable.
* `prefetch_mapped` can advise `WillNeed` on the next layer's range while the current one computes,
  which is the Colibri `couple_prefetch` idea and turns a fault into an overlap.

### What decides whether it works

**Expert locality across tokens.** If consecutive tokens route to overlapping experts, the hot set stays
small and the page cache absorbs it. If routing is close to uniform, every token touches a different 3%
and the working set approaches the whole file — at which point you are reading from disk at disk speed
and the answer is quantization after all.

That is measurable before anything is built: log the routed expert IDs per layer per token over a few
hundred tokens and compute the reuse rate. `moepolicy` already tracks hit rates for the pinned-expert
cache, so the instrumentation mostly exists.

## Why the GPU cannot do this

Per the driver audit, drm/asahi has **no shrinker** — no purge, no evict, no madvise. GPU buffers are
pinned for life. So the paging trick is CPU-only: the kernel can reclaim page cache backing a CPU
mapping and cannot reclaim a GPU allocation. A model larger than VRAM must stream explicitly on the GPU,
and can simply be mapped on the CPU.

That is an argument for GLM-5.2 being a **CPU-path model** on this machine, at 113 GiB/s measured, rather
than a GPU one — and it is the first case where the CPU path is not a fallback but the only option.

## Order of work

1. Measure expert reuse on a model we can already run (Scout, Qwen3-30B). Cheap, and it decides the rest.
2. If reuse is high: GLM-5.2 on the CPU path via mmap, no new quantization needed.
3. If reuse is low: IQ3_XXS support, with a gate against a CPU reference — and note that Q6_K took four
   faults to get right, three invisible to inspection.


## Colibri runs GLM-5.2, and its glm.c answers all three blockers

`colibri-main/c/glm.c` is 6,657 lines of "Motore GLM-5.2 (architettura glm_moe_dsa)". Its header
comment is an implementation guide, and one line removes the blocker I called structural.

### The sparse attention indexer is a NO-OP at short context

> *"Il DSA indexer e' un NO-OP per seq <= index_topk (seleziona tutte le key): qui si usa attenzione
> causale densa -> output identico all'oracolo su prompt corti."*

The indexer selects the top-k keys. When the sequence is **at or below `index_topk`, top-k is every
key** — so dense causal attention is not an approximation of the sparse path, it is *the same
computation*. Colibri validates this against a transformers oracle and reports identical output.

GLM-5.2's `index_topk` is 2048. So **the whole indexer is unnecessary below 2048 tokens of context**,
and above it the model degrades rather than breaking. That converts "structural blocker, no dense
fallback" into "works today at short context".

### The two weighting knobs, exactly

```c
3036:  if (norm_topk) { sum = Σ w; sum += 1e-20; w[k] /= sum; }   // normalize
3037:  for (k) w[k] *= routed_scale;                              // THEN scale
```

**The order is load-bearing and the wrong order is silent.** Normalizing divides by the sum, which
removes any common factor — so scaling *before* normalizing cancels the scale entirely. The model runs,
looks fine, and `routed_scaling_factor` does nothing.

### Residency is far smaller than the file

> *"gli expert (streaming) e la parte DENSA residente … sono tenuti in int8 per-riga + scala … ~17B
> param residenti a int4 ~= 8.7 GB"*

The routed experts stream per-expert from disk; only the dense part, attention, embeddings, lm_head and
the shared expert stay resident. That is **~17B parameters, not 200B+** — which is the `tensorplace`
split arrived at independently, confirmed by an implementation that runs the model.

## Revised status

| blocker | status |
|---|---|
| `ExpertWeightsNorm` | **implemented** — sum, epsilon, divide |
| `ExpertWeightsScale` | **implemented** — after the norm, with the ordering test |
| `SparseAttentionIndexer` | **not needed below 2048 context**; dense causal is exact there |

What remains before a run: the MLA path (`supported: false`, never verified end to end), and reading
`routed_scaling_factor` out of GGUF metadata rather than defaulting to 1.0.

---

# The IQ1_S memory blocker, quantified

Your file is `GLM-5_2-UD-IQ1_S`, six shards, ~25 GiB — about **136B parameters at 1.58 bpw**.

IQ1_S is readable: `gguf.rs` type 19 is a full port of llama.cpp's `dequantize_row_iq1_s`, with the
2048-entry ternary codebook transcribed rather than derived.

**But reading it is not the problem. Keeping it is.**

```
source (IQ1_S, 1.58 bpw)   25.1 GiB
q4 resident                71.5 GiB   2.8× the source
int8 resident             127.1 GiB   5.1× the source
```

Against **measured** GPU ceilings of 63 GiB (M2 Ultra) and 31 GiB (M2 Max) — so the q4 conversion alone
overshoots the Ultra and is more than double the Max.

This is the arithmetic that decides GLM-5.2 on both platforms, and it is not about the architecture at
all. Every gap in `archfeat` could be closed and the model still would not fit, because **the loader
inflates a 1.58-bit checkpoint to 4.5 bits before it ever runs a token.**

## Why the conversion exists, and why it does not apply here

The engine converts to q4 or int8 because those are its *compute* formats — the matvec kernels read
them directly. That is the right trade for Q4_K and Q8_0 sources, where conversion is roughly
size-neutral and buys a kernel that runs.

For IQ1_S it is a 2.8× tax to reach a format the source was chosen to avoid. Someone shipping IQ1_S has
already decided that 1.58 bpw is the acceptable quality point; converting up does not recover the
quality, it only spends the memory.

## What would actually make it run

**A native IQ1_S matvec.** There is none — `iq1s_grid` appears once, in the CPU dequantizer. The kernel
would read the 50-byte super-block directly: 11-bit codebook index per group of 8 weights (8 bits from
`qs`, 3 from `qh`), a 3-bit per-sub-block scale and a sign bit, indexing the ternary grid.

That is more work per weight than Q4_K unpacking, and decode is **latency-bound rather than ALU-bound**
here (≈6% BW, ≈1% ALU) — so the extra arithmetic is close to free while the 2.8× bandwidth saving is
not. The shape of the win is favourable in a way it would not be on a compute-bound device.

**Expert streaming does not rescue it.** It converts experts to q4 on the way in, so it moves *when* the
inflation happens, not whether.

## Order of work

1. **Try it with `LOGSCOPE_TRY_UNSUPPORTED_ARCH=1` anyway**, under 2048 tokens. It will refuse on
   memory rather than on architecture, and the refusal will print the real numbers — which is worth
   having as a measurement rather than the estimate above.
2. If those numbers match, the native IQ1_S matvec is the whole remaining task, and it is a kernel
   gated against the existing CPU dequantizer — **run that gate on hardware**, per `gate-validity.md`.


## The conversion goes through f32, and the precheck does not know

`make_lin` takes **f32 weights** and produces q4 or int8. Every source format is dequantized to f32
first — so the path for an IQ1_S tensor is:

```
IQ1_S  →  f32  →  q4
1.58bpw   32bpw   4.5bpw
```

The f32 intermediate is **20× the source** for the same tensor. Per tensor, on GLM-5.2's largest:

```
token_embd (150k × 8k)   src 0.23 GiB → f32 4.58 GiB → q4 0.64 GiB
output     (150k × 8k)   src 0.23 GiB → f32 4.58 GiB → q4 0.64 GiB
one expert gate/up       src 0.02 GiB → f32 0.33 GiB → q4 0.05 GiB
```

Transient and per-tensor, so it does not multiply across the model — but it does mean **peak memory
during load exceeds the resident total the precheck sizes**. That is the same shape as the UMA
double-count fixed in _376: a plan approved on one figure and executed against another.

For a Q4_K source the intermediate is 7× rather than 20×, and the peak is small enough beside the
resident total that nobody noticed. For a 1.58-bit source it is the dominant term of the load.

**Consequence for the native-IQ1_S-matvec argument:** it removes both costs at once. No f32
intermediate, no q4 inflation, and the weights stay at the size the checkpoint author chose. It is not
a 2.8× saving on top of a working path — it is what makes the path exist.

## Status, both platforms

| piece | state |
|---|---|
| `glm-dsa` architecture recognised | yes, distinct from `glm4moe` |
| IQ1_S readable | yes — llama.cpp port, codebook transcribed |
| ExpertWeightsScale / Norm | implemented, metadata-driven, ordered correctly |
| MLA dims, `kv_b` fusion | read from metadata; per-head fusion fixed in _371 |
| sparse-attention bound | read and reported; dense is exact at or below it |
| attempting a run | `LOGSCOPE_TRY_UNSUPPORTED_ARCH=1` |
| **fitting in memory** | **no — 71.5 GiB resident against 63 / 31 measured** |

Everything except the last is done. The last needs a kernel.

---

# Open: `auto_q4` assigned and never read in the fully-resident branch

`cargo` reports `value assigned to auto_q4 is never read` at `lib.rs:7568`. That line is inside the
**fully-resident** MoE branch — the one whose comment says:

> `moe_stream_pin` stays None, which is what selects the resident path below.

`auto_q4` is read at 7678 (`q4_mode`), and assigned at three places: 7522, 7568 (resident), and 7634
(streaming, alongside `moe_stream_pin = Some(pin)`).

**Two readings, and they have very different consequences:**

1. **Benign.** The branches are exclusive and the compiler is flagging a redundant store the resident
   path never needs. Nothing to fix.
2. **A real bug.** The resident branch falls through to 7634, which overwrites the store *and* sets
   `moe_stream_pin = Some(pin)` — meaning a model that fits fully resident is put on the streaming
   path anyway, contradicting its own log line. That would explain Scout being refused for
   ~50 GiB of host expert storage on a machine where it fits resident.

A brace-depth count from 7568 to 7634 gives −1, which is consistent with either: the branch may have
closed, or an enclosing block may have.

**Do not guess.** Resolving it needs reading the control flow properly, and the cost of being wrong is
the memory path for every MoE model. The check is small: whether the resident branch reaches 7634, and
whether `moe_stream_pin` is `None` when the loader prints `MoE fits FULLY RESIDENT`.

**Quick empirical test:** load a MoE model that fits resident and look for `MoE fits FULLY RESIDENT`
followed by any streaming activity. If the two appear together, reading 2 is correct.

---

# The MLA dims the loader refuses for are all derivable from the tensor shapes

The actual failure, from a real GLM-5.2 preview run:

```
worker: load failed: architecture 'glm-dsa' ships MLA tensors but no
kv_lora_rank in its metadata, so the latent dimensions are unknown.
```

The refusal is honest — the metadata genuinely omits it. **But the tensors carry it.** From the same
run's dump:

```
blk.0.attn_kv_a_norm.weight     [512]           -> kv_lora_rank  = 512
blk.0.attn_q_a_norm.weight      [2048]          -> q_lora_rank   = 2048
blk.0.attn_kv_a_mqa.weight      [6144 x 576]    -> 576 - 512     = 64 = qk_rope_head_dim
blk.0.attn_k_b.weight           [192 x 512 x 64]-> 64 heads over the 512 latent
blk.0.attn_v_b.weight           [512 x 256 x 64]-> v_head_dim    = 256
```

**The norm vector's length *is* the latent rank** — `attn_kv_a_norm` normalizes the compressed KV, so
it has exactly `kv_lora_rank` elements. Same for `attn_q_a_norm` and the query rank. And
`kv_a_mqa`'s output width is `kv_lora_rank + qk_rope_head_dim` by the DeepSeek-MLA convention, so the
rope dim falls out of the subtraction: 576 − 512 = 64.

Nothing here is inferred from the family name. Every number comes from a shape in this file.

## Why deriving beats refusing

llama.cpp resolves missing hparams from tensor shapes routinely — a converted checkpoint that lost a
key is common, and the shapes cannot disagree with themselves. Refusing costs a model that is fully
described; deriving costs a check.

**And the check is free here.** The derived values must be mutually consistent: `kv_a_mqa`'s width must
equal `kv_a_norm`'s length plus the rope dim, `k_b` and `v_b` must agree on the head count, and the
latent in `k_b` must equal `kv_a_norm`'s length. Four shapes constraining three numbers — so a file
whose tensors disagree fails loudly rather than loading at a guessed rank.

That is the opposite of the situation the fused-qkv split was in, where the head counts and the tensor
were the *only* two facts and either could be wrong. Here the tensors over-determine the answer.

## What the same dump confirms about the indexer

```
blk.0.indexer.attn_k.weight     [6144 x 128]
blk.0.indexer.attn_q_b.weight   [2048 x 4096]
blk.0.indexer.k_norm.{weight,bias} [128]
blk.0.indexer.proj.weight       [6144 x 32]
```

The sparse-attention indexer is real and shipped, not a config flag — a small separate attention that
scores keys, with its own k-norm carrying a **bias** (unusual, and worth noting before someone writes
it weightless). `indexer_needed_at` already says it is a no-op below `index_topk`, so a short-context
run does not need these tensors — but they are here, and any implementation should read their shapes
rather than assume DeepSeek's.

## Mixed quantization in one block

`attn_output` and `ffn_gate` are Q5_K, `ffn_down` is Q6_K, the MLA projections are Q8_0, the norms F32.
**Five formats in one layer.** Anything that reasons about "the model's quantization" as a single value
— including the `conversion_cost` advice added in _461, which reads the expert tensors' type — is
answering about one tensor and generalising. For this file that generalisation is wrong five ways.

---

# The MLA ranks are in the tensor shapes, and the loader only looks at metadata

The preview log ends the load with:

```
worker: load failed: architecture 'glm-dsa' ships MLA tensors but no kv_lora_rank in its metadata
```

That is a correct refusal — the latent dimensions genuinely are unknown from metadata. **But they are
not unknown from the file.** The dump printed right below the error contains them:

```
blk.0.attn_kv_a_norm.weight   [512]              → kv_lora_rank = 512
blk.0.attn_q_a_norm.weight    [2048]             → q_lora_rank  = 2048
blk.0.attn_k_b.weight         [192×512×64]       → 64 heads, 512 latent, 192 = qk_nope + qk_rope
blk.0.attn_v_b.weight         [512×256×64]       → 64 heads, v_head_dim = 256
```

A LoRA-style norm vector has exactly rank elements — that is what it normalizes. So `kv_lora_rank` is
the length of `attn_kv_a_norm.weight`, and `q_lora_rank` the length of `attn_q_a_norm.weight`, with no
inference beyond "a norm has one weight per channel".

**The head count and the per-head dims fall out of the same dump**: `attn_k_b` is `[192, 512, 64]` and
`attn_v_b` is `[512, 256, 64]`, so 64 heads, a 512-wide latent, and `v_head_dim = 256`. The 192 is
`qk_nope_head_dim + qk_rope_head_dim`, which metadata splits and the tensor does not — so that one still
needs the file's hparams or a convention.

## Why this is worth doing rather than waiting for better metadata

The refusal message already says the MLA path is implemented and validated and that only the dimensions
are missing. Deriving four of the five from shapes turns "this checkpoint cannot be loaded" into "this
checkpoint loads if the qk split is known" — and the qk split is the one value with a documented
DeepSeek-V3 convention (128 + 64) that this file's 192 matches exactly.

## The confirmed shape of the remaining work

The dump also settles two open questions:

**The sparse indexer is real and has tensors.** `indexer.attn_k`, `indexer.attn_q_b`, `indexer.k_norm`
and `indexer.proj` are all present per block. `archfeat`'s `SparseAttentionIndexer` gap is not
speculative, and `indexer.proj [6144×32]` says the index projection is 32-wide.

**Layer 0 is dense.** `ffn_gate`/`ffn_up`/`ffn_down` appear without an `_exps` suffix, so GLM-5.2 has
dense leading layers before the MoE ones — the interleaved pattern the loader already detects per block
from the tensors present, rather than assuming from the architecture.

**Read from the preview log, not measured** — no toolchain in this container after the reset.
