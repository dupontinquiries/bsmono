# RULESET — invariants the engine harness enforces

Same idea as the game-engine harness: every class of breakage becomes a numbered
rule with a check, so it can't recur silently. The **sandbox harness** here is the
`kernel-validate` binary (`cargo run --release --bin kernel-validate` in
`crates/gpu-gemm`) — it must end in `ALL KERNELS VALIDATED ON GPU ✓`. The
**integration tier** is the real model on Apple/AGX (`--features wgpu-engine`,
watch the `wgpu-engine:` logs). Iteration loop: write/tweak → `kernel-validate`
GREEN → ship → run on the M2 → paste failures → fix + add a rule.

| # | Rule | Why (the break it prevents) | Tier / check |
|---|------|------------------------------|--------------|
| K1 | Every new decode/serving path is validated **bit-exact** against the sequential single-session reference (`generate_kv` / `decode_step` / `forward_kv`), tolerance `0.000e0`. | Batched prefill, batched decode, prefix reuse, the scheduler, and their int8 twins all reduce to "do the same math, grouped differently"; any deviation is a real bug, not rounding. | sandbox · `*_ == per-session`/`== per-request` checks |
| K2 | WGSL dequant uses **dynamic `extractBits` + non-underflowing `select`** for sign-extension — never a dynamic plain shift or an underflowing `if`. | AGX (Honeykrisp) miscompiles dynamic plain shifts; this idiom is the AGX-safe one validated on real silicon. | integration (AGX) · dequant checks |
| K3 | Any 1-D compute dispatch whose element count can scale with sequence length goes through `wg1()` (splits across y when >65535 workgroups); the shader indexes `g.y*nwg.x*64 + g.x`. | The long-prompt crash: a 2617-token prefill made `add/mul/silu` dispatch 83744 workgroups in x, over Vulkan's 65535/dim limit → validation error → process panic. | sandbox · `large 1-D dispatch (>65535 wg)` |
| K4 | Streaming decode accumulates **raw bytes** and emits only the complete-UTF-8 prefix (`decode_bytes` + `str::valid_up_to`); never `from_utf8_lossy` per token. | The garbled-character bug: a byte-level-BPE token that is a partial multi-byte char (Japanese, emoji, …) became `` when decoded alone. | integration · visual; logic in `backend.rs` |
| K5 | The loader requires `<arch>.embedding_length` and rejects unknown architectures with a clear error, never a crash or garbage. | A non-Llama GGUF (LFM2.5) correctly failed with `missing embedding_length` instead of loading wrong. Keeps multi-model serving safe. | sandbox · `load_gguf` error path |
| K6 | `Gpu::new()` requests `adapter.limits()`, not `downlevel_defaults()`. | A 1B model's vocab projection exceeds the default 128/256 MiB storage-binding caps; defaults silently fail the load. | integration · real-model load |
| K7 | Compute pipelines are built once and cached by `(shader_ptr, n_inputs)`; `run`/`dispatch` rebuild only the per-call bind group. | Rebuilding a pipeline per dispatch made the 1B look hung (hundreds of dispatches/token). Pipeline count is asserted stable (19). | sandbox · `pipeline cache` line |
| K8 | int8 KV is **lossy**: validate it *tracks* f32 (rel logit err small, greedy token preserved), and only require bit-exactness between two int8 paths (batched vs sequential int8). | int8 vs f32 will never be bit-equal; demanding it would be a false failure. The real guarantees are "close to f32" + "batching is exact". | sandbox · `int8 KV tracks f32`, `int8 batched == per-session` |
| K9 | Model weights are **read-only** after load and shareable across sessions; per-session state is only the KV cache. | This is what makes many instances cheap (see PREWARM.md) — sharing requires the weights never be mutated during inference. | design · review |
| K10 | All GGUF scalar metadata types (u8..f64, incl. the exotic ones) parse; one unknown type must not fail the whole file. | Real Llama-3 metadata carries float64/int types; an allowlist parser failed the load. | sandbox · `gguf parse` + `--inspect` on the real file |

## Kernel dispatch registry (K3) — 1-D kernels and their element counts

| kernel | elements (1-D) | scales with seq len? | uses `wg1`? |
|---|---|---|---|
| `ADD` / `MUL` / `SILU` | `rows*cols` (= t·d in prefill) | yes | **yes** |
| `SWIGLU` / `SWIGLU_PACKED` | `t·h` (h = FFN hidden) | yes (larger than t·d) | **yes** |
| `ROPE_MH` / `ROPE_MH_ROWS` | `t·dm/2` | yes | **yes** |
| `RMSNORM` | `rows` (one wg per row) | yes, but ÷64·rows is tiny | no (safe) |
| `ATTENTION_MH` | `t·n_heads` | yes, but small | no (safe; revisit at t≳130k) |
| `DECODE_ATTN_MH(_Q8)` | `n_heads` | no | no |
| `ATTENTION_MH_EXT` | `s·n_heads` (segment) | yes, but small | no (safe) |
| `DEQUANT_Q8_ROWS` | `n·c` | with cache len | **yes** |
| `GEMM`/`LINEAR`/`LINEAR_Q8_0` | 2-D `(n/8, m/8)` grid | m up to ~524k ok | n/a (2-D) |
| `ROW_ABSMAX` / `QUANT_PACK` | `n` / `n·c/4` | with batch n | revisit if batched-prefill int8 lands |

> Streaming-window eviction adds no kernel: `decode_step_stream` reuses `DECODE_ATTN_MH`
> over `attend_len` rows; `stream_slot` (O(1), no shift) places K/V so the ring stays
> exact because each K is rope'd at its true position before storage.

> When adding a kernel whose 1-D element count can grow with sequence length or batch,
> route its dispatch through `wg1()` and index with `g.y*nwg.x*64 + g.x` (K3).
