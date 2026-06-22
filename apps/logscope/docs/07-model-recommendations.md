# 07 · Model recommendations & support matrix

*Written mid-2026. The engine implements two architecture families end-to-end: **llama-arch** (Llama 2/3,
RoPE, GQA, SwiGLU, RMSNorm) and **Qwen2** (the same, plus interleaved RoPE and QKV bias), with byte-level
BPE tokenizers and three prompt templates (ChatML, Llama-3, Plain). The validation loop is bit-exact
correctness in-sandbox + real-hardware measurement on an M2 Max (Asahi, Honeykrisp Vulkan, 64 GB). So the
right question for "what model next?" is not "what's best on the leaderboard" — it's "what runs on the two
arches we have, and which new arch buys the most."*

This pairs with the project thesis: how far a *small* model + structure (graph, verification, retrieval,
cheap samples, consensus, human-in-loop) goes before you need a big one. That makes the model choice a
**two-tier** decision, not one model — a fast model for breadth (fan-out, the spin-off checks, the
overnight `:continue-all` frontier) and a stronger one for depth (the authoritative verifier on the
branch you commit to).

## Works today (validated)

- **llama-3.2-1b-instruct (Q4_K_M)** — the breadth workhorse. ~17 tok/s decode, ~6 s prefill on the M2,
  full replies. Llama-3 template. This is the model to fan wide with.
- **Qwen2.5-Coder-14B-instruct (Q8_0)** — the depth model. ChatML. Now correct after the marker-
  neutralization fix (a log line containing `<|im_end|>` was smuggling a control token into the prompt).
  ~2.4 tok/s decode at flush=64; heavier, but it's the one you point at the branch that matters.

## Easy adds — same arch + template, should run with little/no new code

These reuse the existing llama-arch / Qwen2 paths and ChatML/Llama-3 templates. Expect them to load and
run; the only real risk is tokenizer/stop-token edges (the genguard stop-set logic already handles the
known Llama-3 and DeepSeek-distill cases, and the new content-marker neutralization covers ChatML).

- **Qwen2.5-Coder 0.5B / 1.5B / 3B / 7B / 32B** — Qwen2 arch, ChatML. The 1.5B or 3B is the natural
  *coder* breadth model (better at code than llama-3.2-1b for the fan-out); the 7B is a lighter depth
  option than the 14B if you want more tok/s; the 32B is the ceiling if 64 GB allows (Q4/Q5).
- **Qwen2.5 (non-coder) 3B / 7B / 14B** — same arch; general reasoning if you want a non-code branch.
- **Llama-3.1-8B / 3.3-8B-instruct** — Llama-3 arch+template. Llama-3.3-8B is, as of mid-2026, the best
  all-around 8B; a strong general depth model alongside the Qwen coder.
- **DeepSeek-R1-Distill (Llama-8B / Qwen-7B/14B variants)** — Llama/Qwen2 arch under the hood. The stop
  handling already accounts for the distill's `<|end_of_text|>`=128001 case. Gives you a *reasoning*
  branch (visible `<think>` blocks, which the chat UI already folds) to contrast with the coder.

Recommended first three to pull and try, in order: **Qwen2.5-Coder-3B** (coder breadth), **Llama-3.3-8B**
(general depth), **DeepSeek-R1-Distill-Qwen-7B** (reasoning branch). All three should be near-drop-in.

## Needs new architecture work — ranked by payoff

- **Qwen3-Coder-30B-A3B (MoE)** — *highest-value target.* As of mid-2026 this is the standout small-
  coding model and it's a near-perfect fit for the thesis: a 30B-quality model with only ~3B active
  parameters per token, reported >20 tok/s on M3-class Apple silicon — i.e. big-model quality at
  small-model compute, which is exactly the "small effective compute + structure" bet. The work is the
  **MoE layer**: a router (top-k gating) selecting among expert FFNs per token, plus loading/addressing
  the expert weights. RoPE/attention are Qwen-family (largely reuse). This is the one I'd prioritize: it
  upgrades *both* tiers at once and the active-param math is friendly to the M2.
- **Qwen3 / Qwen3-Coder dense (1.7B / 4B / 8B / 14B / 32B)** — Qwen3 changes some details vs Qwen2
  (norm placement, head dims, no QKV bias on some sizes); a moderate arch delta, not a rewrite. Good
  dense coder breadth/depth once the MoE or the dense Qwen3 path lands.
- **Gemma 2 / 3** — different enough to need real work: pre/post-attention RMSNorm, GeGLU (gelu, not
  silu), attention **logit soft-capping**, and a large tied vocab. Strong small models, but budget a
  proper arch pass.
- **Phi-3 / Phi-4-mini (3.8B)** — the only tier-1 option for 8 GB machines; partial-rotary RoPE + its own
  template. Lower priority here given 64 GB, but cheap breadth on constrained hardware.
- **Mistral Small 3 / Devstral** — sliding-window attention + `[INST]` template (a new template family).
  Devstral is agent-tuned (SWE-bench-strong), which is appealing for the verify-loop, but the sliding
  window + template is a chunk of work.

## The two-tier setup I'd actually run

- **Breadth (fan-out, `:fuzzd`, `:continue-all`, spin-off checks):** Qwen2.5-Coder-1.5B or 3B. Fast enough
  to expand many branches; coder-tuned so the cheap branches are still plausible code.
- **Depth (the branch you commit to, the verifier):** Qwen2.5-Coder-14B today; Qwen3-Coder-30B-A3B once
  the MoE layer exists (it should be *faster* than the dense 14B per token while much stronger).
- **Reasoning contrast branch (optional):** a DeepSeek-R1-Distill — its `<think>` trace is a different
  shape of answer, useful when consensus across model *types* matters, not just temperature.

The structural payoff: with the KV-cache tree (the depth-search lever), the breadth model fans cheaply off
shared prefixes and the depth model only runs on the committed line — so a two-model setup costs far less
than running the big one everywhere, which is the whole point.

## Implementation notes for the next arch

- The cheap, compounding prerequisites are already the pattern: per-channel/per-op generality (the
  `dt`-multiplier / specialization work), the tokenizer's verbatim-special handling, and template
  detection by special-token presence. A new arch mostly needs: (1) its norm/activation/attention quirks
  in the forward, (2) its template in the builder (ChatML/Llama-3 already cover Qwen/Llama; `[INST]` and
  Gemma's are new), (3) any new special tokens registered (the GGUF token_type read already does control/
  user-defined tokens), and (4) a `--gate-<arch>` bit-exact check vs a reference forward, same as
  `--gate-qwen2`.
- MoE specifically adds: expert weight loading (N FFNs per layer), a router matmul + top-k select per
  token, and gather/scatter of the active experts. The int8-resident + content-addressed cache path
  generalizes; the router is small. Worth a dedicated `--gate-moe` (router top-k determinism + a forward
  matching a known-good reference within tolerance).
