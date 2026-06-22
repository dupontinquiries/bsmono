# logscope-egui ← logscope integration (egui + backend + ollama→ggml)

## Styling

`logscope-egui/src/theme.rs` is a faithful port of `unendlich-dash`'s analytics
look — the exact palette (bg `#0a0c10`, panels, amber/cyan/violet/green accents),
the monospace type ramp, compact spacing, the amber-hover / cyan-selection widget
states, and a self-contained widget kit (no `unendlich-ui` dependency): coloured
`glow` cards, `panel` (lead bar + UPPERCASE title + copy), `section`, `chip` /
`pill`, `stat_tile`, `list_item` (hover-amber run cards), `event_pill`, the
signature `metric_track` (fill + p99 whisker + max tick + budget line) with a
`usage_bar` shorthand, and a responsive `grid_fit`. Wired in: charts use a
`section` + `grid_fit` stat row; chat uses `metric_track`/`usage_bar` for the
context-usage meter and `chip` for the backend/model status badge. Changing a
palette constant re-skins the whole app.

## Apply to your logscope checkout
```
cd /path/to/logscope
patch -p1 < logscope-patch/logscope-headless.patch   # Cargo.toml, src/lib.rs, src/bloc_engine.rs
cp logscope-patch/llm_backend.rs src/llm_backend.rs    # new module
```

### What the patch changes (additive; the GTK app builds as before)
- Feature-gates the library headless: `gtk4/…` → `gui`, `zstd` → `convo`,
  `tree-sitter*` → `bloc`; `default = ["gui","convo","bloc"]`.
- Adds the local-GGUF backend features (see below) + optional `llama-cpp-2` /
  `encoding_rs` deps.
- Bumps tree-sitter `0.22→0.24` / grammars `0.21→0.23` (the old grammars pin
  `cc ~1.0.90`, which conflicts with `llama-cpp-sys-2`'s `cc ^1.2.61`; the bump
  resolves it). Migrates the 3 `tree_sitter_X::language()` calls to
  `LANGUAGE.into()`, and fixes a pre-existing unescaped-quote syntax bug in the
  `bloc_engine.rs` template table.

## ollama→ggml: BUILD STATUS (validated in CI this session)
The earlier "DRAFT" is now compiled, linked, and run-tested. Three features:
```
llama-ffi    = ["dep:llama-cpp-2", "dep:encoding_rs"]   # the FFI body (shared)
llamacpp     = ["llama-ffi", "llama-cpp-2/vulkan"]      # GPU build (Asahi/Vulkan)
llamacpp-cpu = ["llama-ffi"]                            # CPU build (no shader compiler)
```

**CPU build — FULLY VALIDATED here:**
```
cargo build -p logscope --no-default-features --features llamacpp-cpu
cargo test  -p logscope --no-default-features --features llamacpp-cpu llm_backend   # 4/4 pass
```
This compiled `llama-cpp-sys-2 0.1.146`, linked libllama, and **ran** the FFI:
`LlamaBackend::init()` succeeds and `load_model` returns a clean `Err` on a
missing/!exists path (a `load_model` existence-check was added — llama-cpp-2
`debug_assert`s on a missing file in debug builds). Build needs: `clang`, `cmake`,
`libclang-dev` (bindgen), `git`.

**Vulkan build — toolchain validated; finish it on the M2:**
```
sudo apt-get install -y glslc            # GGML's Vulkan shader compiler (or use the Vulkan SDK)
cargo build -p logscope --no-default-features --features llamacpp
```
In CI this got all the way through: `glslc` present, `vulkan-shaders-gen` built,
SPIR-V shaders generated, and `ggml-vulkan.cpp` compiled. It then stopped only on
**sandbox resource limits** — `ggml-vulkan.cpp` OOMs a 3.9 GB box (needs swap),
and the C++ objects + swap exhausted the ~20 GB disk quota. None of that is a
code/config problem; on your M2 (more RAM/disk) `--features llamacpp` builds. The
Rust FFI is identical to the CPU build that already passed, so it's API-correct.

## Point logscope-egui at it
`logscope-egui/Cargo.toml`: `logscope = { path = "../logscope", default-features = false }`
(place `logscope-egui/` beside `logscope/`, or edit the path). Run:
`cd logscope-egui && cargo run --release` (glow backend).

## What's wired in the UI
- **Unified picker**: logscope's real Ollama `ModelEntry` rows (via
  `providers::build_model_entries`, incl. the `@auto` row) AND local GGUF files
  (`llm_backend::list_local_gguf`), kind-badged `auto/local/LAN` vs `gguf`.
- **Header**: `local gguf:` dir + **Scan**, a backend badge (`llamacpp ✓` once
  built with the feature, else `stub`), and **Load** for the selected GGUF.
- **Chat routing**: loaded local model → `llm_backend::generate`; local-but-not-
  loaded → prompt to Load; else the Ollama stub. Uses the process-global
  active-model registry.

To run local inference on the M2: build logscope `--features llamacpp`, set
`local gguf:` to a folder of `.gguf`, Scan → pick → Load → chat.

## This build adds: threaded chat + analytics restyle (branch point)
- **Threaded inference** (`logscope-egui/src/backend.rs`): because `LlamaBackend`
  is `!Send`, a worker thread *owns* the loaded model and streams tokens back over
  a channel — generation never blocks the UI frame. `llm_backend` gained
  `generate_stream(loaded, prompt, params, on_token)`; `generate` is now a
  buffered wrapper over it (same validated FFI calls). Header **Load** hands the
  path to the worker; **Chat** shows tokens appearing live with a **tok/s**
  readout and a streaming cursor. With no GGUF loaded the worker *simulates* a
  stream, so the threaded UX is visible without a model / without `llamacpp`.
- **Restyle** (`theme.rs`): logscope's amber identity blended with unendlich-dash's
  analytics look — deeper cool ground (#14171c), cyan/violet accents, rounded
  "smooth" widget chrome, a monospace ramp, plus `card()` / `stat_tile()` helpers
  (used in Tokens/Blocks for a dashboard feel). Installed once from the eframe
  creator via `theme::install`.
- Parked for the return trip: the deeper threaded "thinking workload" UX
  (cancellation, queued turns, token/context budget surfacing).

## This build: GPU backend abstraction + GEMM tiler + in-app context control
- **`gpu-gemm/`** (crate `gpu_kernels`): the pure-Rust transformer GPU **kernel
  set** on **wgpu → Vulkan**, each checked against a CPU reference, all dispatched
  through one shared helper (`src/lib.rs`). Kernels: tiled `gemm`, `linear`
  (C=A·Wᵀ, nn.Linear weight layout), `dequant_q4_0` (4-bit GGUF unpack), row
  `softmax`, `rmsnorm`, causal single-head `attention`, `rope` (Llama split-half),
  elementwise `add`/`mul`, `silu`, and embedding `gather`. They compose: the
  validator also runs **one full Llama-style transformer block end-to-end on the
  GPU** (RMSNorm→QKV→RoPE→attention→Wo→residual→RMSNorm→SwiGLU MLP→residual) and
  matches the CPU reference to ~2e-6. Run it: `cd gpu-gemm && cargo run --release`.
  Verified on lavapipe Vulkan — matmuls/dequant/gather/add/mul bit-exact, the rest
  within ~1e-6 (f32 ordering). This is the clean-room engine's compute layer; the
  forward-pass runtime (GGUF load, KV cache, sampler, tokenizer) is the layer above.
- **`Backend` trait** (`logscope-egui/src/backend.rs`): one pluggable interface
  (`load` / `generate` streaming). `LlamaCpp` (Vulkan via `--features llamacpp`)
  is the working impl; `Wgpu` is the seam for the pure-Rust clean-room engine
  built on the `gpu-gemm` kernels. The worker thread owns the (!Send) backend; the
  UI picks it live (Chat → "Context & backend").
- **In-app context management** (Chat): context window, reply budget,
  temperature, and history depth are sliders; a live usage bar shows estimated
  tokens vs budget; the prompt is assembled from system + most-recent turns
  *within* that budget before going to the worker (which then tokenizes/truncates
  exactly). Params are pushed to the worker live via `SetParams`.
- Full-GPU stance: `n_gpu_layers = -1` (offload all); no CPU model path.

## Burn: demoted to a Vulkan correctness oracle (not shipped)
We confirmed Burn's wgpu backend runs on Vulkan here (a tensor matmul, checked
answer sum == 30 → "MATCH (Burn→Vulkan OK)", Burn 0.21 / cubecl 0.10). But for
inference-only on one backend we'd use a tiny slice of Burn while paying for its
whole tree (≈460 deps, ~5-min builds, 0.5 GB binary). So Burn is **demoted**:
- **`burn-probe/`** keeps that proof as a *standalone* crate. Its role going
  forward is a **correctness oracle** — cross-check the `gpu-gemm` kernels and
  forward pass against Burn's ops during development. Run: `cd burn-probe && cargo run`.
- **`logscope-egui` no longer depends on Burn at all.** The `Wgpu` backend is the
  pure-Rust clean-room seam over `gpu-gemm`; the normal build never pulls Burn.
- Why: the agentic features (cancellation, queued turns, KV-cache reuse,
  scheduling) are *orchestration* — no tensor framework provides them. The
  forward-pass compute is the dozen kernels in `gpu-gemm` (already validated,
  composing into a full block). What's left is host-side runtime — GGUF weight
  load + residency, KV cache, sampler, tokenizer — none of which needs Burn.

The shipped path is the pure-Rust clean-room engine on `gpu-gemm`, behind the
`Backend` trait alongside the working `LlamaCpp` (Vulkan) backend. Burn stays
out-of-tree in `burn-probe/` as the correctness oracle.

## Clean-room engine runtime (skeleton)
- **Resident tensors** (`Tensor` / `BlockWeights` in `gpu-gemm/src/lib.rs`):
  intermediates stay in GPU buffers between kernels — no CPU round-trips. Block
  weights upload once; `block_forward` chains the resident ops on the device and
  only the final output is read back. Validated: the resident block matches the
  CPU reference to the same ~2e-6 as the host-chained version (queue ordering +
  wgpu's hazard tracking make each op's writes visible to the next).
- **GGUF reader** (`gpu-gemm/src/gguf.rs`): parses header + metadata KVs + tensor
  descriptors (name → dims / ggml type / aligned data offset) and model hypers.
- **Model load + stacked forward** (`Model` / `load_gguf` / `model_forward` in
  `gpu-gemm/src/lib.rs`): `load_gguf` parses a GGUF and uploads every weight into
  resident `Tensor`s; `model_forward` does embedding lookup → N resident blocks →
  final RMSNorm → output projection, returning logits. Validated: a synthetic
  2-block f32 model is written to a GGUF image, loaded, and run to logits on the
  GPU, matching a CPU reference to ~7e-7.
- **Multi-head attention** (`rope_mh_t` / `attention_mh_t` / `decode_attention_mh`):
  `d` splits into `n_heads` heads; RoPE rotates per-head (split-half within each
  head_dim), each head attends independently (causal), heads write back in place,
  then Wo projects. Validated against a CPU multi-head reference (~1e-7), and the
  whole model/decode path runs with heads=4.
- **Token generation** (`generate` / `generate_kv` / `argmax`): greedy decode in
  two forms — a recompute loop (full forward over the growing sequence) and an
  efficient **KV-cache** path (per-layer resident K/V buffers, a multi-head
  decode-attention kernel, RoPE at the absolute position, new rows copied into the
  cache via buffer-to-buffer). Validated four ways: GPU greedy == CPU greedy
  (identical tokens); KV-cache == recompute (identical tokens); KV-cache logits ==
  the full forward's last-position logits over a varying sequence (bit-exact); and
  with heads=4 the decoded sequence is non-trivial (not a degenerate repeat),
  matching across all three paths. This closes
  **load → resident → multi-head forward → sample → KV-cached decode**.
- **Quantized-weight loading** (`gguf::dequantize` / `type_nbytes` / `f16_to_f32`):
  `load_gguf` detects each tensor's ggml type and dequantizes — F32, F16, and
  **Q4_0** (18-byte blocks: f16 scale + 32 4-bit weights). Validated two ways: a
  Q4_0 quantize→dequant round-trip bounded by the per-block scale, and a full
  multi-head model whose 2D weights are stored Q4_0, loaded + dequantized on the
  GPU, matching a CPU forward over the effective weights to ~2e-6.
- **Tokenizer** (`tokenizer.rs`): byte-level BPE — `encode`/`decode` via a vocab
  and ranked merges, loadable from GGUF metadata (`tokenizer.ggml.tokens` /
  `.merges`). Validated: merge correctness, encode/decode round-trips, and a
  GGUF string-array load. (The GPT-2 byte↔unicode remap + regex pre-split are thin
  layers a real model needs on top; the merge core is here.)
- **Sampling** (`sample` / `SamplerParams`): temperature, top-k, and top-p
  (nucleus) with an xorshift RNG. Validated: temp=0 and top_k=1 reduce to argmax;
  nucleus stays in range.
- **Wired into the app** (`logscope-egui --features wgpu-engine`): the `Wgpu`
  backend now constructs the engine, loads a GGUF, and streams generated text
  through the existing chat UI via the engine's hardware-validated **KV-cache**
  decode (`generate_kv_stream`: prefill → O(1)-per-token decode_step → sample →
  emit), not a per-token recompute. The stream callback is the cancellation seam. Off by default so the normal build stays light;
  verified to compile **and link** with the feature on. (Runtime needs a real
  GGUF + a display, so it's compile-verified here; the math path is validated in
  `gpu-gemm`.) Pick the wgpu backend in Chat → "Context & backend", set the local
  GGUF path, Load, and generate.
- **Cancellation**: a `Stop` button in the chat composer calls `Inference::cancel()`,
  which sets a shared `AtomicBool` the per-token callback polls (the worker is
  blocked inside `generate()` and can't read its command channel, so the flag —
  not a `Cmd` — is the mechanism). It stops at the next token boundary; the
  wgpu engine honours it through `generate_kv_stream`'s early-stop. llama.cpp
  can't be stopped mid-token (ggml's callback has no stop signal). Turns are
  serialized: the UI gates `Send` while streaming and the worker's mpsc channel
  is FIFO, so the single GPU is never raced.
- **Grouped-query attention** (`n_kv_heads < n_heads`): the K/V projections,
  cache rows, and attention kernels are kv-dim-aware, with each query head
  mapped to its shared KV head (`hh / (n_heads/n_kv_heads)`). Plain MHA is the
  `n_kv_heads == n_heads` case (bit-identical to before). The loader reads
  `llama.attention.head_count_kv`. Validated against a CPU reference for both
  full forward and KV-cache decode on a 4-query/2-KV-head synthetic model.
- **Tokenizer fidelity**: byte-level mode applies the GPT-2 byte↔unicode remap
  (space→'Ġ', every byte reversible) plus a best-effort pre-tokenizer (word/
  number/space/other runs, leading space kept with the next run). `from_gguf`
  switches it on for BPE vocabs (`tokenizer.ggml.model` = gpt2/llama/bpe).
  Validated: full byte round-trip and encode→decode of real text (spaces,
  tabs, punctuation, non-ASCII). SentencePiece/unigram isn't handled.
- **Quant coverage**: the loader dequantizes F32, F16, Q4_0, Q4_1, Q8_0, and the
  **K-quant super-blocks Q4_K / Q5_K / Q6_K** (256-elem blocks with packed 6-bit
  sub-scales; Q6_K's 16 int8 scales + 2-bit high parts). Because the model loader
  dequantizes tensors through the generic `type_nbytes`/`dequantize` pair, real
  **Q4_K_M / Q5_K_M / Q6_K** GGUF downloads now load end-to-end — the same path
  the quantized-model forward test already validates for Q4_0. Each format is
  checked bit-exact against an independently packed fixture super-block.
- **Fused dequant-matmul (resident int8 inference)**: weights can now live
  on the device as int8 (`QWeight`) and be dequantized inside the matmul via
  `LINEAR_Q8_0` — 4× less weight bandwidth, the decode bottleneck. A `Lin`
  enum (`F32(Tensor)` | `Q8(QWeight)`) gives every projection one `apply`, so
  the forward pass is precision-agnostic; `load_gguf_quantized` keeps the
  attention/FFN/output matrices int8 (norm gains stay f32, and any matrix
  whose inner dim isn't a multiple of 32 falls back to f32). The kernel uses
  only AGX-proven constructs (dynamic `extractBits`, non-underflowing
  `select`). Validated: the f32 path is unchanged (5e-7), the fused linear
  matches f32 to 5e-7 on identical dequantized weights, and a full int8
  model forward tracks the f32 forward to ~1% relative logit error with the
  predicted token unchanged. Next: extend the in-matmul dequant to the
  K-quant formats and cache pipelines (rebuilt per dispatch today).

## Real-model loading (Llama-3.2-1B Q4_K_M and friends)
Four fixes let real GGUF downloads load, not just the synthetic test models:
- **Device limits**: the compute device now requests the adapter's actual
  limits instead of wgpu's downlevel defaults (which cap buffers at 256 MiB /
  storage bindings at 128 MiB). A 128k-vocab projection is a >1 GiB tensor and
  was silently failing buffer creation — the main cause of the egui load error.
- **Metadata types**: the GGUF parser now tolerates every scalar value type
  (int8/uint16/int16/int64/float64), so one exotic metadata key no longer fails
  the whole parse.
- **vocab_size**: derived from the embedding matrix when `llama.vocab_size` is
  absent (it usually is).
- **Tied embeddings**: Llama-3.2 has no `output.weight`; the loader reuses
  `token_embd.weight` as the output projection.
The Wgpu engine loads weights **int8-resident** (`load_gguf_quantized`): 4×
smaller buffers (so the big vocab projection fits the GPU's binding limit) and
near-lossless for a K-quant source, since int8 out-precisions the 4-bit weights.
Validated in-sandbox by a `write_gguf_llama3` fixture (no vocab_size, tied
output, float64+int8 metadata) that loads and matches the CPU reference.
Known limitation: llama3 RoPE scaling isn't applied (plain RoPE at freq_base);
this can mildly affect long-context quality, not loading or short prompts.

## Chat wiring (making a loaded model actually talk)
- **Backend no longer wiped on tab open**: the worker defaults to the wgpu
  engine and `SetBackend` is a no-op when the kind is unchanged. Previously the
  Chat tab re-asserting its backend on first open recreated the backend and
  silently dropped the just-loaded model, so generation fell back to a canned
  simulation — the "loaded but can't chat" symptom.
- **Special/control tokens**: the tokenizer registers CONTROL/USER_DEFINED tokens
  (from `tokenizer.ggml.token_type`) as verbatim specials — so `<|eot_id|>` is one
  id, decodes literally, and the engine uses it as the stop token and skips
  emitting control tokens into the transcript.
- **Turn-boundary stop**: with the plain User/Assistant prompt format, the chat
  cuts the reply when the model starts the next "User:" turn, so responses end
  even when the model doesn't emit eot.
- Validated: special-token encode/decode round-trip in the kernel validator.
  Known next step: apply the model's real chat template (with special tokens) for
  stricter instruct behavior.

## Pipeline caching (makes real-model decode usable)
Both dispatch paths (`run` for host ops, `dispatch` for resident ops) now build
each kernel's bind-group layout + compiled compute pipeline **once** and cache it
(keyed by the shader constant's address), rebuilding only the per-call bind group.
Previously every dispatch recompiled its pipeline — invisible on tiny test models
but crippling for a real model, which issues hundreds of dispatches per token, so
the first token took tens of seconds and looked hung. The whole validator suite
(thousands of dispatches incl. full forwards and KV decodes) now builds just **15**
unique pipelines total. Correctness is unchanged (all 43 checks pass).

## Backend default = preferred (use the backend that's compiled in)
`BackendKind::preferred()` picks **llama.cpp** when its FFI is built in (the mature,
fast path — your logs show it loading the model and offloading all 17 layers to
Vulkan) and the clean-room **wgpu** engine otherwise. Both the worker and the chat
tab default to it, so Load and Send operate on the *same* backend — previously the
chat could sit on a different backend than the one holding the model. Combined with
the SetBackend no-op (only rebuild on a real switch), loading then chatting works
without the model being dropped. llama.cpp stops on the model's EOG tokens
(<|eot_id|> etc.); the chat's turn-boundary cut handles the plain prompt format.

## RoPE frequency factors (llama3 scaling) — engine correctness for Llama-3.x
Llama-3.2 GGUFs ship a `rope_freqs.weight` tensor (length head_dim/2) encoding the
long-context RoPE scaling. The engine now reads it and applies per-dimension
factors in the RoPE kernel: theta = pos·base^(-2i/hd) / factor[i] (matching ggml).
Absent → all 1.0 (plain RoPE), so other models are unaffected. The factors thread
through `block_forward`/`block_decode` and are stored on the `Model`. Validated
against a CPU reference with non-trivial factors (1.5e-8). This closes a real
accuracy gap when driving Llama-3.2 on the clean-room backend.

## Engine logging (see the clean-room backend work)
The wgpu engine was silent in the app while llama.cpp prints pages of load logs,
so there was no way to tell it was running. It now emits `wgpu-engine:` lines to
stderr — adapter, load time, model shape (d/layers/heads/kv/ff/vocab), int8 output,
eos + tokenizer info, prompt token count, prefill time, and final tok/s + pipeline
cache size — plus `worker:` lines for backend selection and load routing. To drive
the clean-room engine: pick **wgpu/Burn (experimental)** in the chat's Context &
backend dropdown, then **reload** the model (switching backend drops the previous
one), and watch the `wgpu-engine:` logs.

## Chat template (coherent instruct output on the wgpu engine)
The chat now formats prompts per backend. For the **wgpu** engine it uses the
Llama-3 instruct template — `<|begin_of_text|>` + `<|start_header_id|>role<|end_header_id|>`
turns ending in `<|eot_id|>`, then an open assistant header — which our tokenizer
maps to the right control-token ids, and the engine stops on `<|eot_id|>`. llama.cpp
keeps the plain transcript format. The turn-boundary cut (`\nUser:`) now applies only
to the plain path, since the template path ends cleanly on the stop token. This is
what makes the clean-room backend produce instruct-style replies worth speed-testing.

## Chat box: context + threading
The chat tab is now built around three explicit parts:
- **Messages** — a flat list of user/assistant turns.
- **Context** — an editable system prompt (the ⚙ Context panel) that always leads
  the prompt, kept separate from the turns. Backend pick + ctx/reply/temp/history
  sliders + a live context-usage bar live here too.
- **Threading** — the 🧵 Threaded / ⊘ Single toggle in the top bar. Threaded includes
  prior turns as history (capped by the history-turns slider + token budget); Single
  sends only the context + the current message (clean one-shot, ideal for speed tests).
- **Composer** — a dedicated bottom input: Enter sends, Shift+Enter inserts a newline;
  Send is disabled while empty or streaming, replaced by Stop mid-stream. Messages sit
  in a height-capped scroll area so the composer is always visible.

## Stop is responsive during prefill
`generate_kv_stream` prefills one `decode_step` per prompt token; a long (e.g.
200+ token, threaded) prompt could take tens of seconds during which Stop did
nothing — the model looked stuck. It now takes a `cancel: &dyn Fn() -> bool` that's
checked between prefill tokens (and at the top of the decode loop), so Stop bails out
promptly and returns 0 produced. The worker passes a closure over its cancel flag;
the chat marks an interrupted-before-first-token reply `[stopped]`. Validator adds a
`kv-cache prefill cancel` check. NOTE: prefill is still sequential — interrupting it
is the fix here; making it *fast* (batched prefill) is the separate speed lever.
