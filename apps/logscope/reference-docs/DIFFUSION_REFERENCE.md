# Diffusion reference extracts (vendored llama.cpp + colibri) — file:line cited

## llama.cpp `examples/diffusion` (vendored zip, master)
- Mask id: `llama_vocab_mask(vocab)` (diffusion-cli.cpp:171) == our `tokenizer.ggml.mask_token_id` read. Hard-asserts present.
- **shift_logits**: metadata key `diffusion.shift_logits` (cli:182-186), **defaults TRUE when absent** (Dream lineage). Semantics (diffusion.cpp:261-265): shifted ⇒ position i reads logits row i−1 (i==0 → row 0). denoise-bench now mirrors this exactly (`--shift=0|1` overrides).
- Schedules are mutually exclusive: `eps==0 XOR block_length==0` (cli:190). Block mode (diffusion.cpp:118,165-180): fill canvas is all-mask to max_length upfront; blocks left→right; attention always sees the whole canvas. denoise-bench `--block=N` mirrors this; `max_length % block == 0` asserted upstream.
- CFG exists (unconditional buffer refilled with mask, diffusion.cpp:227) — parked, not mirrored.
- Oracle protocol: build `llama-diffusion-cli` on-box, run the same GGUF greedy at fixed steps, diff revealed tokens vs `denoise-bench` at matching (schedule, steps, shift). Divergence localizes to sampler vs forward by comparing per-step argmax first.

## flash_qblock non-causal (spec, do after causal verifies on hardware)
- Causal filter is `if(j>qpos){break;}` at flash_qblock.rs:197 — two occurrences (score + AV loops). Non-causal variant: template both away into a leaked `LazyLock<&'static str>` (stable ptr for the pipeline cache), per the gemm_pipeline SOURCE-TEMPLATED precedent, + one non-causal case on the module's own CPU recurrence reference. The RowAcc fold is causality-agnostic; only the pre-fold filter changes.

## colibri (vendored zip): residency findings that transfer
- Residency-declaration **batching** is the stall killer: 25.9 s → 3.9 s (−85%) at cap16 by declaring once on a queue-attached set instead of per command buffer (SUMMARY.md, E4/E5). wgpu analog: our arena/BO registration churn — same lesson as the size-class pooling commit, different layer.
- Allocation-strategy changes can tax the **load path** independently (+12–13 s expert-disk-load from heap sub-buffers, E4): change residency bookkeeping and allocation separately, never as one experiment.
- `map_of_fd`/`g_maps` is `retain_map`'s origin; expert-as-view is the end state — **blocked on moe-raw going green** (views currently read garbage; 1.645e20 max|diff|).

## IQ-family load path — the finish line (vendored references)
- Measured gap: UD-IQ1_S 0.26 GB/s vs Q4_K_M 1.25 GB/s at load. Cause: Q4_K converts on-GPU (`dequant_q4k`, lib.rs:4402); IQ/K types fall to scalar single-thread `gguf::dequantize`. Interim fix landed: pool-parallel block dequant + per-tensor timing prints (any tensor ≥256 MB reports).
- The finish = the Q4_K precedent applied to the IQ family. llama.cpp ships the exact porting references: `ggml/src/ggml-vulkan/vulkan-shaders/dequant_iq{1_s,1_m,2_xxs,2_xs,2_s,3_xxs,3_s,4_nl}.comp`. Port order: **iq1_s first** — its 2048-entry codebook is already device-resident (`Gpu.iq1_grid`), so the WGSL is unpack + LUT. Scalar semantics cross-check: `ggml-quants.c` — sign rule `qh & 0x8000 ? -IQ1S_DELTA : IQ1S_DELTA` (line 2662; per-nibble form 2701-2703 for iq1_m).
- Endstate is colibri's: never convert. `native_quant_batch.h` keeps batch-major native-quant matmuls that "preserve the exact scalar column accumulation order of the single-token reference kernels" — our `Lin::iq1` expert path is already this shape, and the 703 tree hands it per-expert slices (lin_row_view). The next load run's per-tensor prints will show exactly which tensors still convert; those are the porting queue, largest first.

## LLaDA2.0-mini wiring (spec source: the llama.cpp llada2 PR, user-supplied)
- Arch = Ling-2.0/bailingmoe2 shape under prefix `llada2`; every "new" item lands on existing generic machinery: fused attn_qkv (loader fallback, lib.rs:8838), per-layer dense-lead (expert-presence detection, 9064), shexp (9249), nextn exclusion (mtp_layers), sigmoid+normalize routing (MOE_TOPK mode 1). Non-causal forward is caller-side: `denoise-bench`.
- **Named gap:** `blk.N.exp_probs_b` (no-aux selection bias) is fetched nowhere — shared with the GLM/DSV3 support. Expert selection runs unbiased; throughput and mechanics unaffected, routing fidelity approximate until wired (bias-aware MOE_TOPK sibling const is the shape of the fix).
- Checkpoint: wsbagnsv1/LLaDA2.0-mini-preview-GGUF, Q4_0 9.27 GB — trivially resident. Protocol: `gate-llada2` first (pure CPU), then `denoise-bench <file> 512 1.0 32 --block=32`; expect `shift_logits=false` printed FROM METADATA — if the preview conversion predates the key, pass `--shift=0` explicitly (LLaDA predicts in place; the mirror-upstream default is true). The qwen3-gate qk-norm finding shadows output QUALITY here too until that triage lands.

## MoE prefill is dispatch-bound — measured, with the fix named
- denoise-bench now prints disp/step. Tiny-llada2 (1 dense + 2 MoE layers, 4 experts): 73 disp/step, L-invariant. Scaling the decomposition to mini (19 MoE layers x 256 experts x ~3 dispatches per expert): ~15k disp/step predicted — and the Ultra's measured 3035 ms/step / 15k = ~0.20 ms/dispatch, Honeykrisp's known per-dispatch cost class. The step is launch-bound, not compute-bound (1.1 TFLOP at 2 TF/s would be 0.55 s).
- The fix is one kernel: a ragged multi-expert GEMM (one dispatch per layer per matrix, all experts via row-offset descriptors) — ggml's mul_mat_id. Porting references in the vendored tree: `ggml/src/ggml-vulkan/vulkan-shaders/mul_mm*_id*` and the CPU `ggml_compute_forward_mul_mat_id`. Collapses ~768 expert dispatches/layer to ~2-3; predicted mini step ~3.0 s -> ~0.6-0.9 s (5 -> ~20-25 tok/s at 32-step), and it is the SAME kernel the Scout-verify economics were waiting on (16 rows/expert thin-GEMM problem).
- Verify on the next real runs: mini rerun should print ~15k disp/step; Scout diffstep prints its own count. If the measured ms/dispatch matches 0.2, the model above is confirmed and mul_mat_id goes to the top of the kernel queue.

## Decode profile of record — llada2-mini AR run (egui, M2 Ultra, 2450 tok)
- 5.02 tok/s = the v1 readback ceiling the code documents ("~5-10 tok/s until GPU-side top-k lands"). Decomposition: 199 ms/tok total = 170 sync + 26 submit; 77 submits/tok = 75% PASS-BOUNDARY-forced + 25% downloads (19/tok ~= one router readback per MoE layer). Bind-group cache 0% reused (KV-slice keys cannot repeat).
- Finding: the v2 GPU-routed decode EXISTS (zero readbacks, MOE_TOPK on device, expert-indexed matvecs over stacked weights) and the outer guard PASSED for llada2 — the v2 function declined internally and silently. All bailouts now narrate, said-once; the next decode run names the arm.
- Ranked engine levers, with the profiler's own words: (1) pass sharing — "the engine opens a compute pass per dispatch, and WebGPU permits consecutive dispatches to share one, which is what llama.cpp's Vulkan backend does with a whole graph" — kills the 75%; (2) whatever the v2 decline is — kills the 25%; (3) t>1 expert-indexed batching over the SAME stacked/selw machinery = the mul_mat_id work for the diffusion step and Scout-verify, cheaper than a from-scratch kernel now that v2's descriptor plumbing exists.
- Flag: "autotuned decode tile = 2147614728" — that value is not a tile, it is a packed word or corruption; the autotune cache entry for this GPU wants inspection.

## PREDICTION GRADED WRONG — the diffusion step is compute-bound, not launch-bound
- Measured on the Ultra (718, denoise-bench, mini @ L=512): disp/step = 257 at 10.84 ms/dispatch, step 2786 ms. The ~15k-dispatch model above was FALSE: v1 grouped prefill is coarse-grained at scale, launch cost is ~51 ms of the step, and the other ~2.7 s is thin-expert GEMM work (~1.4 TFLOP at ~0.5 effective TF/s).
- Consequences: (a) the t>1 V2T path's prefill case is retracted — its matvec-shaped workgroups may well be SLOWER than v1's grouped GEMMs at L=512; one opt-in run prices it honestly, and the number decides. (b) V2T's decode case (windowed t=2-8) STANDS: that regime is launch+readback bound per the egui profile. (c) The real levers for the 3 s step, in order: thin-GEMM efficiency (LOGSCOPE_PREFILL_TILED=1 is a free A/B if the grouped path respects it), fewer steps (schedule work), and B>1 fan-out amortizing weight reads across sequences.
- Open observation: 712-tree and 718-tree runs of the SAME deterministic command produced different degenerate text ("The"x100+newlines vs all-newlines). No forward-path math changed between those trees on the v1 route. Either a missed tree delta or run-to-run nondeterminism on this driver — a back-to-back rerun of the same command splits the two in three minutes. (Predates the 727 shader break, so NOT resolved by it — first-run-autotune remains the leading explanation.)

## Cross-engine forward oracle (tools/oracle) — three shadows down
- Protocol: build vendored llama.cpp's LIBRARY only (cmake -DGGML_METAL=OFF -DGGML_VULKAN=OFF -DLLAMA_BUILD_TOOLS=OFF, target llama); gcc oracle-ref.c against it; tools/oracle/run.sh runs both engines teacher-forced on the same f32 tiny (load_gguf_opt(bytes,false) keeps f32 so the FORWARD is what's measured) and compares per-position argmax with per-side margin tie-conditioning. Tinies declare tokenizer.ggml.model="none" — llama.cpp loads vocab-less first-class.
- Results (lavapipe + llama.cpp CPU, 48 positions each, tau=1e-3): qwen2 48/48, qwen3 48/48, qwen3moe 48/48 — zero near-tie disagreements anywhere, meaning the engines agree far tighter than the tie threshold on margins down to 3e-4.
- CLEARED for the sensible-output suspect list: base attention/NEOX-rope/RMS/SwiGLU/unembed; qk-norm-before-rope (the standing gate-qwen3 shadow — whatever that gate fails on lives in quantized paths, larger dims, or its own reference, NOT the f32 qk-norm math); softmax MoE routing + expert FFNs.
- glm4moe 48/48 (separate q/k/v; sigmoid gating + REQUIRED exp_probs_b bias + weights_norm + scale 2.5 + shexp + dense-lead=1 + post_attention_norm): the ENTIRE llada2-shared MoE/routing/norm surface agrees with the reference. Reference facts the refusals taught: glm4moe creates NO per-layer ffn_norm (post_attention_norm serves the pre-FFN role; our g2 fallback already converges on the same tensor); LLM_TENSOR_ATTN_POST_NORM's file string IS "post_attention_norm" (gemma's spelling is canonical); the fused-qkv reference graph SIGILL here was this sandbox's ISA (native f16 ops in flash_attn_ext), guarded by LLAMA_FLASH_ATTN_TYPE_DISABLED in oracle-ref — the plainest reference path anyway.
- REMAINING llada2-only surface: bidirectional attention (already f64-gated + hardware-certified), fused-QKV slicing (hardware-loaded + tiny-llada2 e2e; not reference-comparable here — see the ISA note) and bidirectional attention (f64-gated + hardware-certified). The first shexp-complete llada2 conversion arrives with every shared subsystem pre-verified cross-engine.

## Batched canvases (denoise_stats_b) — machinery green, one fenced RED
- The fan-out shape: b canvases in one forward. Attention gets a per-sequence key clamp (spare dims field, 0 = old behavior, sl <= one tile is bit-exact), RoPE wraps positions per canvas (rope_mh_t_sl), everything else is row-independent. gate-denoise-b proves (re-proved on the LIVING tree after the 727 shader break resolved below — the original recordings of these same claims were corpse comparisons): raw kernel batched == per-half solo BIT-EXACT; qwen2 / qwen3 / qwen3moe full-model batched == solo BIT-EXACT at every tested length. The compute-bound step's rows multiply by b; its weight traffic does not.
- RESOLVED — the RED was never a batching bug. Root cause: the 727 edit renamed the attention
  dims struct field `_c` -> `sl` with a replace-FIRST, and the identical struct line exists in more
  than one kernel. The rename hit a different kernel's copy; the bidir kernel that received the new
  `dd.sl` code kept `_c` -> naga "invalid field accessor `sl`" -> create_shader_module fails as a
  NON-FATAL LOGGED validation error -> the module never exists and every dispatch through it
  silently no-ops. llada2-class files then show zero hidden (exactly-uniform stats, p = 1/256 to
  the bit) and qwen-class shows zero-stats; every denoise-path result from 727 to the fix was a
  comparison among corpses that could still agree with each other. The 8-experiment exoneration
  dossier above is VOID as evidence (each cell compared broken to broken); the pow2 "pattern" was
  pool-recycling luck — which alloc sizes handed the dead kernel's output buffer zeros versus
  garbage — not a property of any batching code. Found by: probe-bidir (direct row-stats
  observation, no gate machinery), git bisect of lib.rs between 726 (alive) and 727 (dead), and
  finally running with stderr VISIBLE — the 2>/dev/null habit hid the naga error all night.
- FIRST 729 HARDWARE LOG (LLaDA2-mini Q4, M2 Ultra): no panic, shader guard silent, deferral's
  no-frame path never fired. Output degenerate as EXPECTED — the checkpoint declares 1 shared
  expert and carries NO shexp tensors (loader line 47), so every token drops an always-on FFN
  contribution; sensible output still blocks on the shexp-complete conversion. Decode profile:
  6.27 tok/s, sync 95.8% of wall, 18 submits/tok at 16 dispatches/submit = ~0.54 ms of GPU per
  dispatch against a ~1 ms/tok bandwidth roofline for ~1B active params — two orders of overhead.
  The v2t fold DID engage (254 downloads/1009 tokens = per-window sampling only, no per-layer
  router readbacks) despite a stale advisory claiming v1; both that message and the pass-boundaries
  verdict arm asserted pre-726/723 architecture and are now measurement-driven. The fused-vs-not
  question the log could not answer is now answered by every future log: the profile line prints
  actual passes vs dispatches.
- Fence LIFTED. On the living tree: gate-denoise-b green x3 across llada2 (full MoE+shexp),
  llada2-L2, llada2-dense, qwen3, qwen3moe, glm4moe at l=24 and l=32; probe-bidir shows real
  margins at every t in 8..64 on all three file families; 799/799 lib tests; oracle 4/4;
  gate-bidir-attn, gate-llada2, gate-rnd1 green. No genuine batched-canvas bug ever existed —
  the seq-clamp design survived first contact with reality intact.
- HARNESS DOCTRINE (stands independently of everything below): a grep-COUNTED pass is void — a
  panic also prints zero FAILs. Only cells that printed a verdict line count.
- RESOLUTION of the 728 "unified suspect": WRONG. Pooled-buffer lifetime explained nothing — the
  probe corruptions, the qwen3moe b==1 self-mismatch, and the pow2 failures were all the one dead
  shader (see the batched-canvases section). The mid-frame-download aliasing theory was a story
  constructed to explain corpse data. Its instruments survive it: LOGSCOPE_POOL_POISON=1 NaN-fills
  recycled buffers at handout (green on the living tree — no stale reads under gate workloads),
  and the return-deferral + scatter zero-fill are KEPT AS HARDENING, honestly classified: the
  encoding-order argument and the accumulator's +=-from-birth assumption are correct by
  construction, but the tree passes with both disabled (LOGSCOPE_NO_DEFER=1 LOGSCOPE_NO_ZERO=1),
  so they are load-bearing for nothing currently demonstrable.
- Probe-safety findings RETRACTED TO UNPROVEN: both "corruptions" were observed only on the
  dead-shader tree and are void as evidence. The mechanisms remain theoretically possible;
  deferral forecloses them by construction; poison is the standing verifier if they ever recur.
- REAL fix found in the wreckage: the v2/v2t GPU routing never applied expert_weights_scale — the
  MOE_TOPK/MOE_TOPK_BIAS dims struct had no scale field while the CPU route multiplies it in
  (moe.rs). Exposed when the regenerated tinies started carrying scale 2.5; the 719-era
  "bit-exact" was honest against a pre-scale-key tiny at scale 1. Fixed by threading
  m.cfg.routed_scale through both kernels; validated: gate-moe-t v1/v2t match within tolerance on
  a scale-2.5 file, which fails if the factor is omitted or double-applied.
- NEW DOCTRINE from the root cause: (a) census over anchors now extends to WGSL struct text —
  identical struct lines exist in multiple kernels and replace-first renames the wrong one;
  (b) probes run with stderr VISIBLE, always; (c) shader compile failures are now FATAL AND NAMED
  at pipeline creation — the guard panics quoting the first shader line and the compiler error,
  and was negative-tested (deliberate corruption -> named panic, restore -> alive); (d) strings-
  grepping the binary for new kernel text is necessary but NOT sufficient — the source was in the
  binary all night while the module died at creation time, which is exactly why (c) exists;
  (e) a multi-bin cargo build with a nonzero error count must be ATTRIBUTED PER TARGET (grep
  'could not compile') before dismissal — gate-bidir-attn silently failed to build from 727 to 730
  (missing the new sl arg) while sibling bins built, its error hid inside counts written off as the
  sandbox's phantom-error quirk, and every verdict it produced in that window came from a STALE
  pre-727 binary (which is also how it "passed" on the dead-shader tree: its binary predated the
  break). Fixed; its first genuinely current run passes on lavapipe; the hardware re-certification
  of the sl-bearing kernel rides on the next on-device gate run.

## Block-diffusion decode reference (chatllm.cpp models/bailing.cpp, LLADA2 registration)
- ATTENTION IS BLOCK-CAUSAL at block_length granularity (default 32): mask[i,j] = 0 when
  block(i)+past_blocks >= block(j), else -INF — full bidirectional within a block and over all
  committed blocks, future blocks invisible. Neither of our modes matches today: whole-canvas
  denoise sees the future; a token-causal windowed decode blinds within-window tokens to each
  other. The sl-clamped bidir attention + KV cache compose into the right topology (window =
  current block, cache = committed blocks) once the grain is 32.
- LOOP: prefill whole-block multiples into KV; tail becomes the block seed, rest mask_id. Per
  block: `steps` iterations (steps = min(steps, block_length), schedule block_length/steps
  commits per step, remainder added to the LAST steps). Each step: forward the WHOLE block; for
  every still-masked position sample via TOP-P (do_sample forced true) getting (pos, token,
  prob); commit the top-N candidates ranked BY PROB. Block full -> emit, n_past += block_length,
  next block all-mask. Logits are read IN PLACE (row i predicts position i) — consistent with
  shift_logits=false for the lineage.
- shift_logits: mainline's converter writes FALSE for the whole llada family
  (conversion/llada.py); its CLI defaults absent->true only because mainline always writes the
  kv. Fork GGUFs omit it; the engine now resolves absent+llada-lineage to FALSE with a loud line.
- Group-limited routing: CPU route implements the reference (sigmoid -> +bias -> per-group
  top-2-sum -> topk_group groups -> top-k over masked biased scores -> UNBIASED weights,
  normalized +1e-20, x scale), proven against a transcription of modeling_llada2_moe.py across
  200 random trials. GPU TOPK kernels select globally and now DECLINE loudly to v1 when groups
  are active; the kernel port is the named next step. Metadata wins including the disable
  (expert_group_count=1 in a file means off; absence is the only case the llada2 8/4 default
  fills).

## The cross-model gibberish: decode autotune had no correctness check (found via DeepSeek)
- DECISIVE CLUE: dense DeepSeek-coder-6.7b degenerates too. It shares no llada2 mechanism — no
  MoE, no diffusion, no partial rope, no shexp — so the live blocker is in a path EVERY model
  takes, and the llada2 fixes (partial rope, shift_logits, shexp, groups, template) were all
  necessary but none was the current cause.
- ELIMINATED IN SANDBOX, each with a new permanent gate: chunked prefill is BIT-EXACT against
  token-by-token decode at 513 tokens across a 512 boundary (gate-prefill-long — the hardware
  prompts were 1658/1897 tokens through 512-token chunks, four chunks, and every previous gate
  used 5-token prompts); windowed decode matches CPU-sampled decode exactly at k=4 and k=10 with
  BOTH the f32 and the int8 embed table (gate-window-q8 — kernel-validate's parity check passes
  EmbedT::F32 while every real run logs "int8 embed table resident", so the q8 gather in the
  window had never been compared to anything).
- ELIMINATED FROM THE REAL VOCAB: the engine's own prompt-tail ids, decoded against the uploaded
  tokenizer.json, read exactly `<role>HUMAN</role>hi there<|role_end|><role>ASSISTANT</role>`.
  Encoder, byte-level BPE, specials and the Bailing2 template are all correct on the real file.
- ROOT CAUSE CANDIDATE, and the first one that explains every symptom: `autotune_matvec` chose
  decode kernels by MEDIAN TIME ALONE. The GEMM tuner has always filtered candidates against a
  reference; the decode tuner never did. A kernel that miscompiles on a driver therefore wins by
  being fastest, and the choice is cached to disk PER GPU — surviving every rebuild and every
  unrelated fix, on that machine only, invisible to every other device. Wrong decode matvec =
  wrong logits from the first generated token, for EVERY model, while prefill (GEMM, validated)
  stays clean. The hardware log shows exactly that: `decode tile = 8 (kernel V2Wg128, cached
  autotune for this GPU)` and `29 decode tiles from cache`, with prefill healthy.
- PROOF THE HAZARD IS REAL, not theoretical: with the new filter, a CLEAN in-sandbox tune rejects
  `V2WgSg tile 8` at rel 2.6 — a candidate returning entirely wrong results, present in the
  candidate set today. Under the old tuner it was one stopwatch away from being cached.
- FIXED: candidates are dispatched once and compared against a CPU f32 reference before any
  timing (tolerance 3e-2 absorbs q8 rounding; a miscompile is O(1)); failures are excluded and
  named loudly with their shape; if every candidate fails, the fallback is V2Wg128 tile 1 with a
  SUSPECT warning. LOGSCOPE_POISON_TILE=<tile> corrupts one candidate on demand so the rejection
  path stays exercisable (verified: poisoning tile 4 rejects all six tile-4 variants and tuning
  still completes). Cache epoch bumped to autotune-v6 — the third time this lesson is paid for:
  A CACHED VERDICT OUTLIVES THE CODE THAT PRODUCED IT, so fixing a prober without bumping the
  epoch never reaches a machine that has already run.

## THE CROSS-MODEL GIBBERISH: pass fusion (my c4-723 default) — reproduced and retracted
- The sample ended the guessing: `generated text = " > > > > >"`, ids [1938, 31690, 31690]. Real
  tokens, stuck repeating — not mojibake (detokenisation is fine) and not a template problem. The
  first token comes from prefill; every decode step after it returns the same argmax.
- REPRODUCED IN SANDBOX, tiny dense model, lavapipe: probe-decode-parity compares prefill's
  last-row logits against decode_step's logits for the SAME token — two paths (GEMM vs matvec)
  that must agree. Default: 0/6 positions disagree, worst rel 8e-8. LOGSCOPE_FAST_DECODE=1: 6/6
  disagree. Bisected: BG_CACHE clean, FUSE_GLU clean, FUSE_DOWN clean, LOGSCOPE_PASS_FUSE=1 ALONE
  reproduces, FAST_DECODE=1 + PASS_FUSE=0 clean.
- MY REGRESSION, shipped in c4-723, which defaulted pass fusion to follow the fast-decode verdict.
  Both failing hardware runs log `fast-decode levers ON (cached A/B verdict)` — i.e. fusion on.
  Default RETRACTED: LOGSCOPE_PASS_FUSE=1 is now the only way in.
- WHY gate-passfuse MISSED IT, the lesson worth more than the fix: it compares FUSED against FUSED
  for internal consistency and never compares a fused decode against the unfused prefill that must
  agree with it. A gate that only exercises the feature it tests cannot see the feature being
  wrong. The parity shape — two independent paths that must produce the same answer — is what
  catches this class, and it is now in the battery under every lever combination
  (probe-decode-parity --gate, run default / FAST_DECODE=1 / individual levers).
- MECHANISM NOT YET PROVEN, so this is a retraction rather than a fix. Leading candidate:
  dims-buffer lifetime. `ring_dims` recycles a fixed ring of uniform buffers and rewrites them with
  `queue.write_buffer`, which materialises at SUBMIT time; deferred (fused) dispatches replay after
  a later dispatch has already rewritten the uniform they read, so an earlier dispatch executes
  with a later one's dims. The ring's wrap-flush protects the immediate-encode path; whether every
  path drains the pending list before recycling is the thing to prove next.
- Also eliminated with permanent gates along the way: chunked prefill is bit-exact across a 512
  boundary (gate-prefill-long); windowed decode matches CPU-sampled decode at k=4/k=10 with both
  the f32 and int8 embed tables (gate-window-q8); multi-slab arena packing is identical to a single
  slab (LOGSCOPE_ARENA_SLAB_MB=1 vs default); the real prompt's ids decode correctly against the
  shipped tokenizer.json.

## "Fluent noise": the chat path never called the diffusion generator
- After the pass-fusion retraction the output became STRUCTURED and still wrong — real words,
  correct punctuation, multilingual drift, no coherence. `diffuse_generate`'s own doc comment
  names that exact failure: a diffusion Model "cannot be AR-decoded into anything but fluent
  noise". The hardware log said it too, in a line that had been there all along: `windowed
  sampling ON (k=4)` — the AUTOREGRESSIVE path. grep across the egui crate found ZERO references
  to diffuse_generate: the chat backend AR-decoded LLaDA2 every time.
- Every fix before this was necessary and none could have produced sense. Partial rope,
  shift_logits, group routing, the Bailing2 template and the pass-fusion retraction all had to
  land for the AR path to even reach fluent noise instead of a repeated token.
- WIRED: the backend now routes `model.diffusion` through diffuse_generate (block denoising),
  emitting the generated tail through the same streaming callback so stop handling, the runaway
  guard and detokenisation are unchanged. LOGSCOPE_DIFFUSION_AR=1 restores the AR path for
  comparison; _STEPS and _BLOCK tune the schedule (default 32/32, matching chatllm.cpp's
  block_length and steps).
- QKV BLOCK ORDER RESOLVED, from chatllm.cpp's BailingMoe2 converter: a plain contiguous
  split([n_head*hd, n_kv*hd, n_kv*hd]) — exactly what this loader assumes. (The v1 Bailing
  converter interleaves per head, which is why the warning was right to exist for the family in
  general.) The same file confirms rope dim = head_dim * partial_rotary_factor = 64. The loader
  line now states the verification instead of warning about it.
- FALSE ALARM FIXED: the MoE routing cross-check probed blk.0 for shexp tensors, and blk.0 is the
  DENSE-LEAD layer on every leading_dense_block_count>0 architecture — so it printed "NO
  ffn_*_shexp tensors" directly above the loader's own "shared expert ACTIVE on 19 layer(s)". It
  now probes the first layer that actually has experts. Same bug class the graft tool hit; two
  checks contradicting each other in one log is worse than neither.

## 737 hardware test: the diffusion path ran, and every embedding gather was DROPPED
- The routing engaged (`DIFFUSION model — routing generation through diffuse_generate`) and produced
  660 tokens at 2.04 tok/s, with the log full of: `Each current dispatch group size dimension
  ([65536, 1, 1]) must be less or equal to 65535`. A whole-canvas forward gathers n*d elements at
  once — at d=2048, a 2048-token canvas is 4194304 elements = EXACTLY 65536 workgroups, one past
  the limit. Over-limit dispatches are rejected as NON-FATAL logged validation errors, so the
  embeddings never ran and everything downstream was computed from stale buffers. Same silent-no-op
  class as the 727 dead shader, third appearance of that pattern.
- WHY ONLY NOW: AR decode gathers ONE row per step (grid 32). The diffusion path was the first code
  to gather a whole canvas, so the limit had never been approached.
- FIXED: GATHER reads @builtin(num_workgroups) and both call sites go through wg1(), which already
  spills into y. Test: a 65536-row gather (exactly the failing count) with reversed ids, probing
  rows 0/1/40000/65529/65535 — proves the y-arithmetic is right, not merely legal.
- SYSTEMIC: `dispatch()` now PANICS, naming the shader's first line, when any grid dimension exceeds
  65535. wg1() spills for kernels that read num_workgroups; a kernel that does not cannot be spilled
  and must be fixed rather than dispatched flat. Census: the wg1 sites (SWIGLU, ADD, …) are 2D-safe;
  the plain `.div_ceil(64), 1, 1)` sites are not — GATHER was the one the diffusion path hit, and
  the guard now names any other the moment it is reached.
- SECOND FINDING, same log: the load line said `template=Bailing2` but the prompt ended
  `assistant<|end_header_id|>\n\n` (Llama3) with 0 specials — a second prompt-building path
  bypassed the resolved template, and the markers went in as ordinary words. The prompt diag now
  raises TEMPLATE MISMATCH whenever a prompt carries `<|` marker text that tokenises to zero
  specials on a model that registers specials. The builder that bypasses live_template() is the
  next thing to find; the alarm makes it visible in one line instead of by decoding tail ids by hand.

## The diffusion loop itself was wrong in two ways (chatllm.cpp as spec)
- STEPS WERE GLOBAL, NOT PER BLOCK. The reference runs `steps` iterations for EVERY block,
  committing block_length/steps positions per step with the remainder on the LAST steps. Our loop
  ran `steps` iterations across the whole fill with k = ceil(n_fill/steps), so at n_fill=660,
  block=32, steps=32 it revealed 21 of a block's 32 positions from ONE forward. Twenty-one
  positions committed off a single distribution is near-independent per-position sampling — real
  words, correct local grammar, no global coherence. Exactly the observed output. Each commit is
  supposed to condition on the previous ones; ours barely conditioned at all.
- FUTURE BLOCKS WERE VISIBLE. The reference masks them (-INF, block-causal at block_length
  granularity); ours forwarded the entire canvas, so every position attended to hundreds of
  still-MASK positions. Fixed by truncating the forward at the current block's end, which gives
  identical visibility for the rows being committed without touching the attention kernel, and is
  cheaper. (A real block-causal mask is still needed if we ever want to commit across blocks in one
  forward.)
- COST, the honest tradeoff: forwards = n_blocks * steps_per_block, each over the whole prefix,
  because this path has NO KV CACHE — the reference re-forwards only the block and reads the prefix
  from cache. The reference ideal (steps == block_length, one token per commit) would be ~640
  whole-prefix forwards for a 660-token reply: minutes per turn. Default is 8 steps per block
  (4 tokens per commit, vs the old loop's 21) and the loop prints its block count, per-commit token
  count and total forwards at startup. CACHING THE COMMITTED PREFIX IS THE NAMED NEXT WORK and is
  what makes the faithful 1-token-per-commit schedule affordable.
- GATES: the schedule arithmetic is pinned as a unit test (even split, remainder on the LAST steps,
  every schedule fills its block exactly, and the specific 21-vs-1 regression); gate-diffuse runs
  the generator end to end on a tiny llada2 and checks every fill position is committed, no MASK
  survives, and ids stay in vocab. The block-causal check reports VACUOUS rather than PASS when the
  tiny's argmax is constant, which it is on random weights.

## BIDIRECTIONAL ORACLE: the denoise forward is VERIFIED (tools/oracle/llada2_ref.py)
- The causal path has had a cross-engine oracle since 724 (llama.cpp, 4 models x 48/48). The
  bidirectional path never had one, because llada2 has never landed in mainline llama.cpp — so
  every diffusion result to date was compared only against itself. Closed by transcribing
  inclusionAI's modeling_llada2_moe.py into NumPy: same f32 GGUF, no attention mask, four stats per
  position in denoise_stats' own format.
- RESULT: 7 of 7 llada2 variants agree at t=8/16/32 with p_top rel 0.00e0 and entropy rel 1.8e-7 —
  full MoE, all-dense, groups-disabled, no-shexp, separate-qkv, 2-layer, and the shift-kv-less
  variant. Every argmax identical, no near-tie tolerance needed. The QUANTISED load (int8/q4
  resident, exactly as the app loads it) also agrees 16/16 against the f32 reference.
- WHAT THAT ELIMINATES, with evidence rather than argument: the fused-qkv split, per-head QK-norm
  and its ordering before rope, partial rope's rotary block and pass-through lanes, GQA head
  mapping, bidirectional softmax, sigmoid routing with the selection bias, group-limited top-k,
  weight normalisation and the 2.5 routed scale, the always-on shared expert, dense-lead layers,
  the output projection, and q4 residency on this path. None of these is the remaining fault.
- MARGIN DEFINITION, recorded so the columns are never misread again: the engine's third stat is a
  LOGIT difference (ROWSTATS_F32 emits w1-w2 on raw logits), not a probability difference. The
  first run of this oracle looked like a 200x discrepancy until that was pinned down —
  p_top * logit_margin reproduces the probability margin exactly.
- WHAT REMAINS for the real model: the prompt framing (the 737 log built a Bailing2-detected
  prompt with Llama3 markers and 0 specials), the commit schedule (fixed in 739 — it was revealing
  21 of a 32-token block per forward), and anything that only appears at real scale (d=2048, 20
  layers, vocab 157k) which no tiny can reach. tools/oracle/run-bidir.sh is the standing check.

## END-TO-END DIFFUSION ORACLE + the prompt builder that bypassed the template
- LOOP ORACLE (tools/oracle/run-diffuse.sh): a reference block-diffusion loop over the already
  verified NumPy forward, compared against diffuse_generate's generated ids EXACTLY. Covers what
  run-bidir.sh cannot — block order, per-block step count, commits per step, which positions win,
  block-causal visibility and shift_logits. 9/9 identical across three schedules on three model
  variants including a shift_logits=true tiny built for the purpose.
- IT CAUGHT TWO THINGS ON THE WAY. (a) The reference itself was wrong first: diffusion.shift_logits
  is written as the STRING "false" and bool("false") is True in Python, so the reference emitted the
  LAST PROMPT token's argmax as the first fill token. The engine was right; the oracle was wrong —
  worth recording because the reverse is the usual assumption. (b) A real nondeterminism in the
  engine: on a fresh block EVERY masked candidate has identical confidence (identical context), so
  ranking on raw f32 left commit order to reduction noise — the same model and prompt could commit
  different positions run to run. Both sides now quantise confidence to 1e-6 and break ties by LOWER
  INDEX, which is reproducible and reads left-to-right.
- HARNESS DOCTRINE, paid for again: the first run of this comparison printed "IDENTICAL" nine times
  from a bad `cd` — both sides were EMPTY and empty equals empty. run-diffuse.sh now refuses when
  either side is empty and says so. Same family as the panic-counted-as-pass lesson from 728.
- PROMPT BUILDER FIXED. The 737 log's Bailing2-detected model was handed a Llama3-marker prompt with
  zero specials because the CONTINUATION builder (the "expansion" path) read `self.template` instead
  of `live_template()` and knew only ChatML and Llama3/Llama4 — every other family fell through to
  bare "System:/User:" text, and Bailing2 had no arm at all. It now has one (open assistant turn, no
  trailing <|role_end|>, so the partial resumes in place), reads the detected template, and says so
  loudly once when a template has no resume arm. The non-wgpu Gemma guard read `self.template` too;
  it now reads the same resolved value.

## Scale probes: the grid class is closed, and the output projection is 75x oversized
- WIDE PROBE MODELS (tools/tiny-variants/gen_wide.py): d=2048, head_dim=128, 16/4 heads, L=2, tiny
  vocab — real-scale SHAPES at ~95 MB, so grid arithmetic that scales with t*d and t*heads matches
  the real model without real-scale weights. probe-grid then exercises every op on the diffusion
  path at t=2400 under catch_unwind and reports which exceed the 65535 workgroup limit.
- RESULT: all clear. gather_t spills correctly at 76800 workgroups (the op that WAS dropped on
  hardware), swiglu_packed at 192000, rope/rmsnorm/rowstats far below. The 65536 gather was the
  only one waiting; the fatal grid guard now names any future one at the moment it is reached.
- OUTPUT PROJECTION ROW WINDOW. The diffusion loop reads stats for the CURRENT BLOCK only (~32
  rows) while the projection ran over the whole canvas: at t=2400, d=2048, vocab=157184 that is
  1.55 TFLOP and a 1.5 GB logits tensor per forward, ~75x discarded. denoise_stats_rows windows the
  final norm, projection and row-reduction to the rows actually read; the layer stack still runs
  over ALL positions because attention is bidirectional. Verified exact: windowed stats equal full
  stats sliced across six ranges, the window cannot leak (RAII guard), and the end-to-end diffusion
  oracle is still 6/6 identical with the loop wired to use it. The FLOP and memory reduction is
  arithmetic; the wall-clock win is unmeasured here and belongs on hardware.
- WHY NO SPEED NUMBER FROM THE SANDBOX: lavapipe is a software rasteriser. Timing the projection
  alone at n=32768, k=2048 gave 0.38s at m=2400 and 5.32s at m=32 — 14x SLOWER for 75x less work,
  steady over three samples. That inversion is evidence about lavapipe's per-dispatch and
  thread-scheduling costs, NOT about Honeykrisp, and it is exactly the kind of number that must not
  become a default. A chunked small-batch route (8-row groups through MATVEC_SMALL_Q8_0, which is
  hard-limited to 8 rows by its accumulator array) measured slower still there (11.9s), so it ships
  OPT-IN as LOGSCOPE_SMALL_CHUNK=1 with an equivalence test proving it changes no numbers. The A/B
  belongs on hardware: LOGSCOPE_SMALL_CHUNK=1 vs unset, one pair of runs.

## The diffusion path could not stop, could not be cancelled, and showed nothing
- Three defects the hardware logs implied but no gate covered, all from one omission:
  `diffuse_generate` took neither a stop set nor a cancel callback nor a streaming callback.
  1. NO EARLY STOP. Every reply filled its entire ceiling. At n_fill=660, block=32, steps=8 that is
     21 blocks x 8 = 168 whole-prefix forwards even when the model finishes in two blocks — the
     caller then threw the rest away. Typical replies were paying the maximum.
  2. NO CANCEL. Nothing checked it, so a diffusion generation could not be interrupted at all; the
     only escape was killing the app, which is exactly what happened on the 40k-submit run.
  3. NO STREAMING. The reply appeared only after every block finished — minutes of silence, and the
     runaway guard (which lives in the emit callback) never ran until it was too late to matter.
- FIXED via diffuse_generate_cb(stop, cancel, on_block); diffuse_generate is now a thin wrapper so
  existing callers and the oracle are unchanged. Committed blocks are scanned for eos/stop and
  generation ends at that token; cancel is checked between steps (worst case one forward); each
  block's tokens go through the SAME `step` callback the AR path uses, so detokenisation, stop
  handling and the runaway guard behave identically on both paths.
- GATED: a stop token inside a block ends generation at that block and is not itself emitted;
  cancel shortens generation; streamed tokens equal the returned reply exactly. The end-to-end
  diffusion oracle still matches the reference 6/6, so the new exits did not disturb the schedule.

## Diffusion turns were paying for two setups they never use
- Read back from the 737/739 logs: both print `full prefill (first turn for this model) in 12.50s`
  and `windowed sampling ON (k=4) — int8 embed table resident (307 MiB, 0.15s)` IMMEDIATELY BEFORE
  the line announcing that generation is routing to diffuse_generate. Neither is used by that path:
  diffuse_generate_cb takes only the raw prompt ids and re-forwards the block-truncated canvas each
  step, reading no KV cache; the int8 embed table exists solely for the autoregressive sampling
  window, which never runs. So every diffusion turn paid ~12.5s of prefill and 307 MiB of
  allocation for nothing, on top of the generation itself.
- SKIPPED, on exactly the condition that routes to diffusion: model.diffusion AND
  LOGSCOPE_DIFFUSION_AR != 1 AND a mask id is present (diffuse_generate refuses without one). Both
  fallbacks — the AR env and the missing-mask refusal — decode autoregressively and still need the
  prefilled cache; skipping there would decode from an EMPTY one, which is precisely the silent
  garbage-from-nothing class this hunt keeps finding. The predicate is duplicated across two call
  sites, so it is pinned by a test covering all five combinations rather than left as a comment.
- The empty Session is still constructed so downstream borrows stay valid; it is simply never
  filled, and the loader says so in one line.

## Why there is no prefix K/V cache (negative result), and the mask that would enable one
- THE OBVIOUS OPTIMISATION IS UNSOUND. Each denoise step re-forwards the whole prefix, so caching
  the prefix K/V once per block and reusing it across that block's steps looks free. It is not:
  attention here is fully bidirectional inside the truncated window, so PREFIX POSITIONS ATTEND
  FORWARD into the current block. When the block's tokens change — the entire point of a step —
  the prefix activations change too. Measured directly: identical 12-token prompt, block filled
  with MASK vs filled with token 7, prefix logits differ by 8.3e-4. Caching would freeze stale
  values and corrupt every subsequent step silently — the same class as the dead shader and the
  dropped gather.
- The reference (chatllm.cpp) can cache because its mask is block-causal in BOTH directions
  (mask[i][j] = -INF when i + past/block < j): row block i never looks into a later block, so the
  prefix genuinely is fixed. Our forward truncation reproduces that for the rows we COMMIT, but not
  for the rows we would want to cache. The prerequisite for a K/V cache is therefore the MASK, not
  the cache.
- PINNED AS A TEST (kv_cache_soundness): prefix rows still depend on the current block. It fails
  the moment that stops being true — i.e. exactly when caching becomes safe and this note is stale.
- BLOCK-CAUSAL MODE IMPLEMENTED, OPT-IN. ATTENTION_MH_BIDIR_PAR gained a bcz field: when nonzero,
  a query's key range is clamped to the end of its own block. Verified to (a) make prefix rows
  independent of the block's contents, and (b) actually change the stats versus bidirectional, so
  it is not a silent no-op. It ships OFF because it CHANGES OUTPUT — a more faithful model of
  attention, not an optimisation — and its payoff (the K/V cache) is not built yet.
- CENSUS NOTE: ATTENTION_MH_EXT_PAR and ATTENTION_MH_BIDIR_PAR share identical struct D text. Both
  copies were grown together and the EXT host's dims array widened to match, even though only the
  bidir kernel reads bcz. This is exactly the 727 failure mode (replace-first renamed one kernel's
  struct and silently killed the other); the census habit caught it before the build.

## Block-causal alignment: blocks are anchored at the FILL START, not position 0
- Found by auditing what I shipped last turn rather than by a failing test. The bcz mask derived a
  row's block index as i/bcz from position 0, but the diffusion loop's blocks begin at n_ctx, and a
  prompt length is rarely a multiple of the block size. Arithmetic, no model needed: at the real
  n_ctx=1718, bcz=32 the first fill row was limited to keys <1728 while its own block ends at 1750
  — MASKED AGAINST 22 OF ITS OWN 32-TOKEN BLOCK. The reference never hits this because it REQUIRES
  past % block_length == 0 (CHATLLM_CHECK in bailing.cpp); our loop has no such constraint, so the
  mask has to carry the origin. Fixed: the kernel takes bco, rows >= bco index blocks from there,
  and rows < bco (the prompt) see exactly the prompt and no block.
- THE EMPIRICAL CHECK WAS VACUOUS AND NEARLY REPORTED A FALSE BUG. Perturbing a block token and
  watching a row's stats reported "CANNOT SEE" even for the correctly-aligned case, because a
  random-weight tiny's logits move by ~1e-8 under any masking change — below any threshold worth
  asserting. The mask formula is therefore pinned as ARITHMETIC (block_causal_alignment: every fill
  row sees exactly its own block across n_ctx in {0,1,8,10,31,32,33,1718} x bcz in {1,4,32}; prompt
  rows see exactly the prompt; bcz=0 is unrestricted). Same lesson as the empty-output oracle: a
  check that cannot discriminate must be recognised as vacuous, not read as a result.
- TRUNCATION vs BLOCK-CAUSAL, measured: 1.07e-8 worst p_top delta on the tiny, ~40x above f32
  epsilon at that magnitude, 0/4 argmax differ. So they are NOT identical — truncation lets prefix
  rows attend forward into the block, and layer 2+ carries that into the block's own predictions —
  but the magnitude cannot be assessed on random weights. Note the loop oracle CANNOT settle this
  either: diffuse_ref.py uses truncation too, so both sides share the assumption. Making the
  reference model the true mask, then comparing, is the outstanding work.

## Block-causal attention is now VERIFIED against a reference that models the same mask
- The gap named last turn is closed. llada2_ref.forward takes (bcz, bco) and applies the
  REFERENCE's mask — blocks anchored at the fill start, row block i sees column block j only when
  i >= j, prompt rows mutually visible and blind to every fill block. Until now both sides
  truncated, so the oracle was structurally blind to exactly the difference under investigation.
- RESULT (tools/oracle/run-blockcausal.sh): 10/10 across two model variants and five prompt
  geometries, ALIGNED and UNALIGNED, every argmax identical, p_top rel ~2e-7 — the same order as
  the unmasked path's agreement. The engine's block-causal implementation, including the fill-start
  anchoring fixed in 746, matches a faithful transcription of the reference's mask.
- A NEAR-MISREADING WORTH RECORDING: the first comparison showed p_top rel ~1e-4 under block-causal
  versus 0.00e0 unmasked, which looks like a real systematic discrepancy. It was PRINT GRANULARITY:
  both sides printed 6 decimals, and at p_top ~0.004 one unit in the last place is 2.5e-4 relative,
  so a sub-ulp difference read as 1e-4. Raising both to 9 decimals collapsed it to 2e-7. The
  unmasked case had rounded identically by luck, which made the contrast look meaningful. Rule:
  before believing a small discrepancy, check it is not smaller than the resolution it was measured
  at — the same family as the vacuous checks caught in 741 (empty output) and 746 (1e-8 sensitivity).
- STILL OFF BY DEFAULT. Verified-correct is not the same as verified-better: block-causal changes
  output, and whether it improves a real generation cannot be judged on random-weight tinies. What
  changed is that turning it on is now a decision about matching the reference rather than a leap —
  LOGSCOPE_DIFFUSION_BLOCK_CAUSAL would wire it into the loop, and its payoff (a sound prefix K/V
  cache, per the 745 negative result) is the reason to want it.

## The diffusion loop was GREEDY, and temperature never reached it
- The reference SAMPLES. chatllm.cpp forces do_sample with top_p, samples every masked position via
  `sampler->sampling(logits, vocab_size, &prob)`, and ranks the commit candidates by the SAMPLED
  token's probability. Our loop took each position's argmax and ranked by p_top — a different
  algorithm, and greedy denoising's characteristic failure is repetitive output.
- WORSE, IT WAS SILENT: diffuse_generate_cb took no sampler at all, so the operator's temperature
  did nothing on a diffusion turn. Every hardware log says "temp 0.35" on a line above a generation
  that was strictly deterministic argmax.
- FIXED via diffuse_generate_sampled(sp, rng). CORRECTION to this entry as first written: it
  claimed reusing crate::sample() made "top_k/top_p/min_p/repeat-penalty behave identically across
  both paths". That was FALSE for the repeat penalty — sample() ignores it entirely, and the
  autoregressive path applies it via `pick`, which penalises and then samples. So the first version
  of diffusion sampling left repeat_penalty silently INERT, on the one path whose observed failure
  mode is repetitive output. Now calls `pick` with the prompt plus every COMMITTED token as
  history (masked positions hold the mask id and are not real context), and ranks commits by the
  sampled token's UNPENALISED probability so commit order tracks model confidence rather than
  history. The sampling tests could not have caught this: they use penalty 1.0, where the two
  routes are identical by construction, so a separate gate asserts a heavy penalty changes output. Sampling
  needs a distribution, not a 4-stat reduction, so denoise_logits_rows returns raw logits for the
  windowed rows — affordable only because of the row window: ~20 MB for a 32-row block at vocab
  157k, against 1.5 GB for the whole canvas. The two changes compose exactly.
- GREEDY IS BIT-PRESERVED (sp=None or temperature 0), which matters because the end-to-end
  diffusion oracle is pinned to the greedy path: it still matches the reference loop 6/6. Gated
  both ways — greedy unchanged through the new entry point, and sampling that reproduces on a seed,
  diverges across seeds, stays in vocab, and does NOT coincide with greedy at temperature 1.
- LOGSCOPE_DIFFUSION_GREEDY=1 restores argmax for an A/B on hardware.

## c4-748 shipped a build break, and my verification could not have caught it
- `stop_set` did not exist — the variable is `stop`, bound from self.stop_ids. A one-word slip, but
  the interesting part is why every check I ran said clean: I verified with
  `cargo check -p logscope-egui`, NO FEATURES, while the entire wgpu path — including the diffusion
  code being edited — sits behind `#[cfg(feature = "wgpu-engine")]` (backend.rs:77). The compiler
  never saw the lines I wrote. Same vacuity class as the empty-output oracle (741), the
  1e-8-sensitivity probe (746) and the sub-ulp comparison (747): a check that cannot fail on the
  thing under test is not evidence.
- tools/check-all.sh now compiles gpu-gemm, logscope-egui with wgpu-engine, and the operator's exact
  `--features wgpu-engine,voice`. Run it before claiming a build is clean.
- WARNING WALL, 102 -> 24 in logscope-egui. The bulk was float_literal_f32_fallback, a lint that
  does not exist on this sandbox's rustc 1.96 and so was invisible here: every instance in the
  operator's log is a `Stroke::new` width literal, which is f32 by definition, so 274 sites across
  70 files were suffixed `_f32` blind — safe because the suffix is a no-op where the literal was
  already f32. Also fixed with local verification: 3 unused imports, 9 unnecessary `mut`, 3 unused
  variables, 9 doc comments on macro invocations, redundant parens, and two function-item casts.
- gpu-gemm 4 -> 1 warning. route_trace had ZERO call sites (superseded by route_trace_tok, which
  carries the token id) so LOGSCOPE_ROUTE_TRACE was already served entirely by the other variant —
  removed. attention_mh_ext is dead but KEPT with an explicit note, because ATTENTION_MH_EXT_PAR
  shares its `struct D` text verbatim with the live bidir kernel: that is the 727 footgun, and it
  is better documented than silent.
- A COMMENT CORRECTED, not just a warning silenced. The `auto_q4` dead-assignment site carried a
  long note inferring from the lint that "control flow DOES reach the streaming branch", and
  reasoning onward about models streaming experts they had already escaped. That over-reads the
  lint: "value assigned is never read" is a liveness fact meaning the RESIDENT path never consumes
  auto_q4, not that the streaming branch is reachable from there. The real test is the runtime guard
  further down, which prints an explicit BUG line if a fully-resident model reaches streaming setup
  — no hardware log has ever contained it. Guard kept, speculation removed, assignment deleted.

## The 0.37 tok/s run: three findings, one of them my own 256x
- TEMPLATE MISMATCH FIRED ON A FRESH TURN, and the 738 alarm is what caught it: the prompt carried
  Llama3 markers ("<|end_header_id|>") with ZERO specials on a Bailing2-resolved model. Root cause is
  NOT the continuation builder fixed in 741 — it is `live_template`, the OVERRIDE that build_prompt
  reads via `live_template.unwrap_or(template)`. The three Evt::Loaded handlers set `self.template`
  and then `return`, skipping the update loop's refresh, so a value left over from a PREVIOUSLY
  loaded Llama model survived the model switch and BEAT the new model's detection. All three load
  sites now set both. (My 741 note said live_template "prefers detection" — it does not, it prefers
  the override. Corrected.)
- 892957502.9x. The phase-balance verdict divided by a SKIPPED prefill: c4-744 stops prefilling for
  diffusion models by design, so prefill_ms_per_tok arrives as microseconds and the ratio explodes.
  The existing guard only caught exactly zero. A skipped phase is not a fast phase; below 1e-3
  ms/token the comparison is refused with that reason, pinned by a test using the real numbers.
- THE 256x, AND IT WAS MINE. 276003 dispatches across ~18 forwards is 15333 per forward, which is
  exactly 20 layers x 256 experts x 3 (gate/up/down) — the v1 grouped path issuing per-expert work.
  The token-axis path that collapses this to ~60 dispatches was DECLINING, because c4-734 made both
  GPU-routed paths bail whenever group-limited routing was active, on the correct-but-slow reasoning
  that the kernels selected globally. That decision cost 256x in dispatches on this model.
  MOE_TOPK_BIAS now does group limiting itself — per-group top-2 sum, keep the tg best groups, top-k
  inside them — and both declines are removed. gate-moe-t passes on the GROUPED tiny, meaning the GPU
  route agrees with the CPU route that run-bidir/run-diffuse verify against the HF reference.
  Struct D is shared by both topk kernels and was censused, not replace-first.
