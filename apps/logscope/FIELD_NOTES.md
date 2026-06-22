# Field notes — strange & interesting things

Hard-won quirks from building and debugging the pure-Rust wgpu GGUF engine. Written so
future-us doesn't re-learn them the hard way. The regression tests named below encode each
lesson so it can't silently return.

## The subnormal-f16 bug (the 13-round one)

**Symptom.** Llama-3.2-1B produced word-salad / a single repeated token, while every
SmolLM2 (135M, 360M, any quant) was coherent on the same engine.

**Root cause.** `f16_to_f32` decoded *subnormal* half-floats 4× too small — the exponent
bias for the subnormal branch was off by 2 (seeded `e=-1`, biased by `127-15`, landing two
exponent steps low).

**Why it hid for so long.** It only bites when a value is a subnormal half (< ~6e-5). K-quant
blocks store their scale `d` as f16. A model's weight magnitude sets that scale:
- SmolLM2 (d=576/960) has trained weights ~0.18 → block scale `d` ~1e-4 → **normal** f16 → fine.
- Llama-3.2-1B (d=2048) has weights ~0.02 (init scales as 1/√fan_in) → `d` ~1e-5 → **subnormal**
  → decoded 4× small → every weight shrunk → dead FFN, tiny tied embedding, washed-out logits.

So the bug was invisible to every kernel test, every synthetic model, and every small model.
It needed a *large-d, small-weight* real model to surface.

**The diagnostic that cracked it.** Per-role weight RMS grouped by quant type (`--qcheck`).
In the 1B, Q6_K read ~4× *under* Q4_K for the same role (e.g. `ffn_down` 0.004 vs 0.016),
but in the 360M the two types agreed exactly (0.178 vs 0.179). Q6_K carries an extra 8-bit
sub-scale, so its super-block `d` is smaller and goes *deeper* subnormal — the smoking gun.

**Lesson.** Validate dequant by **magnitude against a known-good reference model**, not by
"finite and plausible range." A uniform 4× shrink looks completely plausible in isolation.
Guards: `f16_tests::subnormals_and_normals`, `kquant_subnormal_tests::{q4_k,q6_k}_subnormal_scale`.

## RoPE in GGUF llama is interleaved NORM, not split NeoX

`convert_hf_to_gguf.py` permutes the Q/K projection weights so that GGML's NORM rope
(rotating pairs `2i, 2i+1`) reproduces HuggingFace's behaviour. If you implement split-half
NeoX rope (pairs `i, i+half`), attention is subtly wrong — coherent-ish at first, garbage at
depth. All rope paths must match in lockstep. Guard: `rope (interleaved NORM)` suite check.

## "Q4_K_M" is a target, not a guarantee

Community "Q4_K_M" GGUFs pack tensors as a *mix*: Q4_K and Q6_K, but also Q5_K, and even
**Q5_0 / Q5_1** (ggml type 6/7) for some tensors in small models. The quantizer chooses per
tensor; the filename is aspirational. New quant types show up unannounced — supporting the
custom backend is ongoing dequant maintenance. (This is the single biggest argument for a
llama.cpp compute backend under the orchestration layer.)

## When a test "diverges," suspect the test

A CPU-vs-GPU forward check "failed" at `rope_base=500000` — looked like a real engine bug.
It was the **CPU reference** hard-coding `base=10000` and ignoring the model's base. At small
base both used 10000 and agreed; at large base only the reference was wrong. Cost a full round
chasing a phantom. The engine handles base 5e5 correctly. Guard: the scale sweep now drives
`m.base` through both paths.

## The anchor (prefix fork) is bit-exact

Seeding a session from a cached prefix KV and prefilling only the suffix produces output
**identical** to a full prefill — `max|Δ| = 0.0`, same greedy tokens. So prefix caching and
multi-instance state-sharing cost *nothing* in quality: it's a KV buffer copy instead of a
re-prefill. This is the correctness floor under the prefix-trie cache and continuous batching.
Guard: `anchor: prefix-fork logits/tokens == full prefill`.

## head_dim is the invariant; group and d are not

1B, 360M, 135M all have **head_dim = 64** despite d = 2048 / 960 / 576. What varies is the GQA
group (4 vs 3 vs 3) and width. A test that fixes head_dim will miss head_dim bugs; a test that
only does group≤2 misses group-4. The scale sweep deliberately covers group 3/4 and head_dim
64/128 at the 1B's exact 32/8 shape.

## What will never run here

`arch=lfm2` (Liquid — conv/attention hybrid with `shortconv` blocks) and `arch=gpt2`
(DialoGPT — learned position embeddings, no RoPE) are not llama-family. The engine implements
RoPE + RMSNorm + SwiGLU + GQA only. `--inspect` prints the arch; `--probe`/load panics with
`missing embedding_length` on non-llama files. Use llama-arch models (SmolLM2 *is* llama).

## Performance quirks

- **Token rate ramps 0.1 → 0.6 → 1.2 → 2+ tok/s** at the start of decode. Two causes: (1)
  one-time WGSL pipeline compilation on the first dispatch of each kernel — decode uses
  `DECODE_ATTN` kernels that prefill never touches, so the first decode tokens pay it; (2)
  GPU clock/DVFS ramp under sustained load (outside our control, same as llama.cpp). Prewarm
  fixes (1) by compiling everything in the load window.
- **The engine is naive**: per-op dispatch with activation round-trips, no kernel fusion, no
  persistent resident activations. ~2 tok/s for a 1B on an M2 Max vs llama.cpp's 50–100+.
  Prewarm smooths the *start*; steady-state throughput needs fusion + resident activations.
- **lavapipe** (the sandbox's software Vulkan) runs compute fine but can't present a window
  and is slow. It's enough to bit-validate kernels; real runs want a hardware GPU.

## Small models give confident, wrong answers

The 135M/1B will happily emit `sudo mdadm --status` to "check NVMe firmware" or `dmesg` on
macOS. That's model capacity, not an engine fault — the engine is faithfully running them.
The capacity guide's 8B is where answers get good. Don't debug the engine over bad advice;
check coherence/grammar (engine) separately from correctness (model).

## The debugging method that actually worked

Eliminate by **measurement against a working reference**, not by theory or code-reading. The
ladder that found the subnormal bug: `--probe` (per-layer hidden RMS — found the frozen
mid-stack) → per-sub-step trace (attn vs ffn contribution — found the dead FFN) → `--qcheck`
(per-role RMS by quant type — found Q6_K 4× under Q4_K) → the f16 subnormal branch. Each step
narrowed the space; none of it came from staring at the dequant code, which had been
"audited" as correct line-by-line (and *was* correct — the bug was one layer down, in f16).

## Decode throughput: it was submit count, not compute (258 → 1 per token)

A decode token is ~250 tiny 1-row kernels. The naive engine gave each its own command
encoder + `queue.submit` — **258 CPU→GPU flushes per token** on the 1B (16/layer × 16 + 2),
measured by the `GPU_SUBMITS` counter in the validator's decode-profile line. Only **one**
blocking sync per token (the logits readback), so activations already stayed GPU-resident
through all layers — the round-trip worry was unfounded; the cost was flush *overhead*.

Fix: a **recording frame**. `begin_frame` opens one command encoder; `dispatch` and the cache
copies record into it (keeping their bind groups + dims buffers alive in the frame); the
terminal `download` appends the readback copy and flushes everything in **one** submit. wgpu
inserts barriers between compute passes in a single encoder, so cross-layer read-after-write
is preserved — which is why the change is **bit-exact** (gqa/anchor/batching/greedy checks all
`max|Δ|=0.0`). Result: **258 → ~1 submit/token**, zero output change.

Deliberately *not* framed: `prefill_batched`. Its intermediates are `[t,d]` (megabytes for a
500-token prompt) and a frame holds every op's resources resident until flush — framing it
would spike memory. Decode intermediates are `[1,d]` (kilobytes), so framing those is free.
Sequential suffix prefill (`session_prefill` → `decode_step`) *is* framed, since it's 1-row.

Next lever is kernel fusion (rmsnorm+matmul, the attention path) — now that submit count is ~1,
per-op compute and dispatch setup are the floor, not queue flushes.

### Addendum: 258→1 submits gave ZERO speedup — submits weren't the wall

Frame batching cut submits/token 258→1 (bit-exact) but tok/s didn't move on the M2. So the
per-submit overhead the count implied was being absorbed cheaply by the driver — I optimized a
number without measuring its wall-time effect. The `--bench <gguf>` profiler then split a decode
token's wall time and settled it: **resource-setup (bind groups + dims buffers) is sub-1% of
wall; gpu-sync (the poll waiting for the GPU to execute the ~210 dispatches) is ~all of it.**
Confirmed on both a synthetic 4-layer model and the real 135M, and lavapipe-on-135M ≈ M2-on-1B
(~2 tok/s) — both GPU-execution-bound.

So decode is GPU-bound on the dispatch *work*, not on submit count or CPU setup. The real levers,
in order: (1) a dedicated **m=1 mat-vec kernel** — the generic tiled GEMM dispatched at one row
wastes most threads and reads weights with poor coalescing; (2) **fuse** QKV into one matmul and
fold the residual add into the matmul output, to cut ~210 dispatches/token. The lesson (again):
a count is a hypothesis, not a measurement — `--bench` before optimizing, not after.

### The app's 2 tok/s vs the bench's 7 tok/s: decode attention used ONE core

`--bench` on the M2 read 7.04 tok/s for the 1B at short context — but the app runs at 2 tok/s
because it decodes at 533+ context. The gap is decode attention: the `DECODE_ATTN_MH` kernel
dispatched `(n_heads.div_ceil(64),1,1)` = a *single* workgroup, so all 32 heads ran as 32 SIMD
lanes on one GPU core (the M2 Max's other ~37 cores idle), each thread scanning the whole KV
cache serially. Negligible at ctx 8, dominant at ctx 533. The matmul floor (~139ms, context-
independent) is the *other* lever; attention-over-KV is what makes the app specifically slow.

Fix (rule-based by shape, per the operator's call to stop hardcoding kernel configs):
`DECODE_ATTN_MH_PAR` runs **one workgroup per head** (uses every core), 64 threads split the KV
rows for QK + softmax via a shared-memory tree reduction, then split head_dim for a coalesced
V-weighted sum. `decode_attention_mh` selects it when `l ≤ 4096` (scores fit in shared memory)
and falls back to the serial kernel above that. Bit-faithful: the V sum keeps serial order, max
is order-free, only the softmax denominator reduction reorders (≤6e-7, argmax/tokens unchanged) —
all decode/anchor/batching checks stay green. Speedup only shows on real multi-core hardware at
long context; lavapipe (software, ctx-8) shows no change, as expected.

### Confirmed: parallel attention took the 1B from 2 → 7.74 tok/s at 544 context
M2 `--bench … 64 512`: 1B 7.74 tok/s at ctx 544 (was ~2 in the app) — actually faster than its
own short-context 7.04, i.e. the long-context penalty is *gone*. 135M 14.2, 360M 9.5. The fix
was the one-workgroup-per-head attention kernel; the matmul floor (~129ms ≈ 7.7 tok/s) is now
the ceiling. Side effect: the bench caused screen artifacts (shared GPU; long single submits +
full-core saturation starve the compositor) — bounded via `FLUSH_EVERY` (see TODO).

### m=1 mat-vec: same math, then coalesced
The int8 matmul `LINEAR_Q8_0` is dispatched (n/8, m/8, 1) with an 8x8 workgroup - at decode (m=1)
only the row-0 threads work, 8 of 64 lanes active. Two mat-vec iterations, both routed via
`linear_q8_0_t` (m==1 -> mat-vec, else GEMM - rule-based by shape like the attention kernel):
  1. Occupancy (one thread per output, flat 64-wide, bit-identical): M2 1B 7.74 -> 8.23 tok/s (+6%),
     smaller models flat. Small win => the GEMM-at-m=1 was MEMORY-bound on strided weight reads
     (each thread a different row = uncoalesced), not occupancy.
  2. Coalesced split-k (`MATVEC_Q8_0`, shipped): one workgroup per output, the 64 threads split k
     so consecutive lanes read consecutive weights/activations (coalesced - bandwidth-optimal),
     then tree-reduce. 2D grid (col = wg.y*gx + wg.x, gx=min(n,32768)) so the vocab projection
     (n=128256 > 65535 workgroup-limit) fits. The reduction reorders the dot sum (~1e-6 logits)
     but argmax is stable - gqa/greedy decode stay token-exact (0.0) vs CPU, anchor 5.96e-7.
     This converts the memory-bound floor toward real throughput; needs M2 bench vs 8.23.

### The floor: sub-linear batched decode + the --workers bench
Many warm branches (the whole speculative-pool / human-voting picture) are only affordable
because decode breadth is sub-linear here. The substrate was already built and validated:
`decode_batched` runs the six weight matmuls once per step for the whole batch (an m=N GEMM —
each weight read once for all N workers, only the per-session attention loops), `prefill_prefix`
+ `seed_from_prefix` fork a shared prefix into each worker (prefill once, KV-copy per worker),
and `run_batched` / `run_fleet` do continuous batching grouped by shared prefix. Correctness is
gated by `batching+anchor == per-request generate` (0.0) in the GPU suite.
`--workers <gguf> [Nmax=8] [steps=48] [prefix_ctx=512]` measures the curve: N workers forked
from one prefix, stepped together, reporting step-ms, aggregate tok/s, per-worker tok/s, and
'vs separate' = (N × the N=1 step)/(the N-batch step). 1.0x = linear (no sharing); higher = the
weight reads are amortizing. The win is per-worker tok/s holding near the N=1 rate as N grows.
Can't measure real scaling in-sandbox (lavapipe is software); needs the M2. Next abstraction
question: a stateful WorkerPool facade (admit/step/collect, keeps branches warm) vs the current
one-shot run_batched — the facade is what the app/voting bridge will drive.

### WorkerPool: the stateful seam (floor meets everything above)
`run_batched`/`run_fleet` are one-shot — they run a queue to completion. The branch/voting
picture needs a pool driven a tick at a time so the caller can react between steps. `WorkerPool`
(lib.rs) is that: `new(gpu, model, sampler, eos, max_batch, max_seq, prefix)` prefills the shared
prefix once; `admit(id, label, suffix, max_new)` queues a branch; `step()` admits into free slots,
emits one token per active branch, retires finished, and batch-decodes the survivors together
(sub-linear), returning a `StepReport {admitted, emitted, finished}`. The differentiators for the
vision: `park(id)` pauses a branch keeping its KV alive in `warm`, and `revive(id)` resumes it
with NO recompute (a pointer move) — so a losing branch can be kept warm and brought back on a
late human vote cheaply. `retire`, `drain_done`, `branch_emitted`, `counts` round it out. Greedy
output per branch is bit-exact to `run_batched` (suite gate `WorkerPool == run_batched` = 0.0,
exercising the max_batch-2 backfill path). This is the object the egui session and the voice/vote
bridge both drive: fast voters steer which branches get `revive`d/`retire`d between `step()`s;
the pool never blocks on them. Next: a heterogeneous variant (a draft+target pair for speculative
decode) and wiring `StepReport` into the chat UI as live per-branch progress.

### Speculative decode: the free-lunch first part (verify_forward + speculative_generate)
The "racing + comparison + downstep" idea made rigorous and quality-preserving. Two pieces:
- `verify_forward(m, cache, tokens, start)` — the target consumes a whole drafted block in ONE
  forward (causal among the new tokens, attending the full prefix), returning logits for EVERY
  position, appending K/V at `start`. Built on `block_prefill_ext`; the reusable verify primitive.
- `speculative_generate(target, draft, prompt, n_new, k)` — draft proposes k tokens; target
  verifies in one forward; accept the longest matching prefix, correct at first divergence, +1
  free bonus token when all k accept. Output is bit-identical to the target's own greedy
  `generate_kv` — the draft only changes speed, never the result. Suite gate
  `speculative decode == target greedy` = 0.0 at K=1,2,4 (a deliberately-different smaller draft
  forces accept/reject/bonus to all fire; the batched verify also bit-agrees with per-token decode
  on argmax). The speedup lands on low-entropy spans (docs/boilerplate/repetitive code) where the
  draft is right in bulk — automatic per-token downstepping, no classifier, no quality tradeoff.
  Constraint: draft + target must share a tokenizer/vocab (so a SmolLM2 family pair, not
  SmolLM-drafts-Llama). Correctness-first details to optimize next: a persistent draft session
  (currently rebuilt per round) and folding the correction token into the next verify (currently a
  separate decode_step). Then: a draft+target speculative mode inside WorkerPool, and an M2 bench
  of tok/s vs K and vs draft acceptance rate.

### Speculative decode — efficiency pass + the --spec bench
Made the correctness-first loop fast without touching the output (bit-exact gate stayed 0.0 at
K=1,2,4 — the proof the optimization is sound, since output is target-greedy by construction
regardless of how the draft is bookkept):
- PERSISTENT draft session (`session_feed` resyncs it): the draft is prefilled once, then each
  round rolled back to the accepted length and fed the correction token — instead of rebuilding
  and re-prefilling the whole context every round (was O(ctx)/round, now O(accepted+1)/round).
- `speculative_generate_stats` records rounds/accepted/emitted for the bench. (Still-open
  refinement: fold the correction into the next verify to drop the per-round extra decode_step.)
`--spec <target> <draft> [n_new=64] [K=4] ["prompt"]`: tokenizes a real prompt, times baseline
target-greedy vs draft+target speculative, prints tok/s, speedup, identical-output, and mean
accepted draft length. Sandbox smoke (135m drafts 360m, both vocab 49152): identical-output=true
("Paris." correct), mean accepted 1.2/4 — but 0.39x on lavapipe (software renderer runs the
batched verify serially with no amortization, and 135m→360m is a weak low-acceptance pair). Like
--bench/--workers, lavapipe can't show the win; the M2 can. Speedup needs: real GPU (the m=K
verify amortizes) + a strong same-family pair where the target is much costlier than the draft and
the draft is often right (ideally SmolLM2-135m drafting SmolLM2-1.7B) + predictable spans
(docs/boilerplate) where acceptance is high. identical-output=true means it's always free quality.

### Small-m coalesced kernel (the verify was on the slow path)
M2 measurement of 135m→360m speculative diagnosed it: the verify forward (m=K=4) cost ~3.6 target
decodes, not ~1, because `linear_q8_0_t` routed everything m>1 to the strided `LINEAR_Q8_0` GEMM —
only m=1 (decode) got the coalesced mat-vec. `MATVEC_SMALL_Q8_0` extends the coalescing to 2<=m<=8:
one workgroup per output, 64 threads split k, each thread reads a weight value ONCE and accumulates
it into all m activation rows, then reduces per row. So the memory-bound weight read amortizes
across the small batch, like m=1 but for K rows. Rule-based routing now: m==1 → mat-vec, 2..8 →
small-batch mat-vec, m>8 → GEMM (prefill). Token-exact across every small-m path (batching+anchor,
WorkerPool batch-2, speculative K=2/4 all stay 0.0). Serves two callers: the speculative verify
block AND batched decode at small N (so the WorkerPool/run_batched small-batch case speeds up too).
Can't measure on lavapipe; the M2 verify(m=K) should now be ~1 weight-read pass instead of ~3.6.

### Model-pairing reality (from M2 --spec runs)
The engine is llama-architecture only (SmolLM2 135m/360m are llama-arch so they load; Qwen3.5,
SmolLM3 are different graphs → "missing embedding_length"). Speculative needs (a) same vocab and
(b) a big cost gap. 135m→360m can't win: 135m is only ~1.5x cheaper than 360m, so K draft steps
cost more than the target forwards they save (measured 0.30-0.40x even oriented correctly:
target=big, draft=small). The fitting Llama pair on a 32GB M2 is Llama-3.1-8B-Instruct-Q4_K_M
(target) + Llama-3.2-1B (draft) — same 128256 vocab, ~8x gap, ~5GB+0.8GB. 70B is ~40GB Q4 → won't
fit. With that pair + this small-m verify kernel + (next) folded correction, speculative should
finally clear 1.0x. Still-open speed refinement: fold the correction token into the next verify
(2 target forwards/round → 1).


## Multi-turn [stopped] / 0 tok/s (two distinct bugs, fixed)
Symptom: after the first reply, later turns produced 0 tokens and showed "[stopped]".

1. **Generation budget was tied to n_ctx.** `n_new = (n_ctx - prompt_len).min(max_tokens)` floored
   to 0 whenever the prompt was as long as the context slider — so a long prompt silently produced
   nothing (the 1B's "up to 0" in the logs). Fixed: `n_new = max_tokens`; the KV cache is sized to
   `prompt_len + max_tokens`, so n_ctx now governs only how much *history* the UI packs in, never
   the reply length.

2. **Anchor reuse could seed decode-written rows.** The anchor is the previous turn's session;
   its shared prefix with the next turn can run *past* the old prompt into the tokens that turn
   *generated* (those rows were written by per-token `decode_step`, not a batched prefill). A new
   kernel-validate gate ("anchor across prefill→decode boundary") shows this is bit-exact at small
   scale (argmax matches, max|d|=3e-6), but it's the one reuse path the suite hadn't been guarding,
   and per-token vs batched KV can drift at 4B scale over a 600+ long prefix. Fix: the anchor now
   records its batch-prefilled length and reuse is capped to that region — we only ever seed
   prefill-originated rows (exactly what the anchor gates validate). The system prompt (the big
   shared chunk) is still reused every turn; only the prior reply is re-prefilled as suffix.

If a *pruned/distilled* model (e.g. elm-turbo-4B) still stops immediately on turn 2 after these,
that's the model genuinely emitting eos — the reuse math is provably identical to a full prefill,
so the cleaner multi-turn test is a stock Llama-3.2-1B/3B.


## Multi-turn fragility, round 2 (batched suffix + self-heal)
After the budget fix, turn 2 (full prefill) generated fine but a later REUSE turn still produced 0
even though l was capped to the pure-prefill region (so it was NOT decode-row seeding). Root cause:
the reuse path prefilled the divergent suffix with per-token decode_step, which differs from the
batched full-prefill kernel by ~1e-6 — enough to tip a near-tied eos once a small model is
degenerating at 1000+ tokens of context. Two changes:
- Suffix is now prefilled with the BATCHED kernel (`verify_forward` / block_prefill_ext at offset
  l), the same path a full prefill uses. New gate "anchor batched-suffix reuse == full prefill"
  measures max|d|=1.9e-6 (vs 3.1e-6 for per-token) with argmax matching.
- Self-heal: if a reuse turn produces 0 tokens with no cancel, the backend retries the turn once
  with a clean full prefill. If full prefill also yields nothing, the model genuinely stopped.

Remaining lever (the user's intuition, correct): a small pruned model degenerates in long context
regardless of engine correctness. Keep prompts short — lower history turns / n_ctx for the small
model, or use a stock (non-pruned) Llama-3.2 — to stay in its coherent regime.


## Long-context OOM + auto-continue (M2 Ultra)
Stock-model multi-turn now works (Llama-3.2-3B: turn1 512 tok, turn2 404 tok — fixes held). Two
new things prompted by pasting a huge context that locked the machine (40-86GB, one core pegged,
GPU "Failed to allocate BO VMA" → OOM):

1. **Chunked prefill (the OOM fix).** `prefill_batched` runs `block_prefill` over the WHOLE prompt
   at once, so attention scratch is O(T²) — a ~40k-token paste tries to allocate tens of GB and
   OOMs the device. New `prefill_chunked` processes the prompt in 256-token blocks via
   block_prefill_ext at increasing offsets; peak attention scratch is O(chunk·T), same result.
   Gate "chunked prefill == batched" is BIT-EXACT (max|d|=0.00e0 at chunk=1/4/8/16). The backend's
   two full-prefill paths (initial + the self-heal retry) now use it. A long context is slower but
   no longer catastrophic.

2. **Auto-continue past the token cap.** A reply that produces the whole max_tokens budget hit the
   cap (vs stopping on eos) and is almost certainly cut off mid-file. The backend reports the count;
   the chat now resumes IN PLACE — `build_continue_prompt` re-emits the turn with the assistant
   message left OPEN (partial text, no eot) so the model picks up exactly where it stopped, and
   streaming appends into the same bubble. Auto up to MAX_AUTO_CONTINUE (4) times per reply, then a
   manual "⏭ Continue" button (also shown when auto-continue is toggled off). Reset on each new send.

Caveat: chunked prefill bounds MEMORY, not total compute — a 40k-token context is still ~O(T²)
work, so it'll be slow. The history window (threading / n_ctx) is still the lever for keeping a
small model fast and coherent.


## Long-context OOM, round 2 — the KV cache, not the attention
The chunked-prefill fix bounded attention scratch, but the device still OOM'd on a huge paste —
this time BEFORE any "prompt N tokens" line, i.e. inside session_new, before compute. Cause was a
regression from my own budget fix: dropping the n_ctx clamp on n_new also dropped the ceiling on
`cap = prompt_len + n_new`, so session_new allocated a KV cache sized to the WHOLE paste. The KV
cache is O(prompt) memory (this 3B: 28 layers × 1024 kv-dim × 2 × f32 ≈ 229 KB/token), so a ~60k
paste demands ~14 GB up front → OOM.

Fix: n_ctx now bounds the whole sequence (its real job). The prompt is capped to `n_ctx - n_new`
(hard ceiling MAX_CONTEXT=32768); on overflow we keep the head (system prompt) + the most recent
tail and drop the middle — standard context-window shift. So the KV cache is bounded by n_ctx and
can't OOM. The n_ctx slider now goes to 32768 (was 8192) so long context is opt-in on big machines
(KV ≈ n_ctx × 229 KB; default 4096 ≈ 0.9 GB). Chunked prefill still bounds attention for the capped
prompt. A pasted file longer than the window is trimmed (with a log line) instead of crashing.


## Checkpoints, rollback, crash recovery + context-increase nudge
Made conversation safety first-class (motivated by the OOM crashes):
- **Autosave**: every completed turn writes the conversation + checkpoints to
  ~/.local/share/logscope/chat_autosave.json (serde_json, best-effort). A crash mid-generation
  loses nothing.
- **Recovery**: on launch, if an autosave exists and the chat is empty, a "Recover previous
  conversation (N messages)?" banner offers Restore / Dismiss.
- **Checkpoints + rollback**: a checkpoint is auto-saved BEFORE every send ("before: <preview>"),
  plus a manual "📌 Checkpoint" button. A "🕘 History (N)" panel lists them with Restore.
  Rollback is NON-destructive — it snapshots the current state ("before rollback") first, so you
  can roll forward again. Bounded to MAX_CHECKPOINTS=30 (oldest pruned).
- **Context-increase nudge**: when the next prompt is estimated to overflow the window (so the
  backend would trim it), an amber banner above the composer offers a one-click "Raise to N"
  that bumps n_ctx to fit (capped at 32768).
Persistence is serde_json (added to logscope-egui only; the engine stays dependency-pure).
Round-trip + cur-clamp (corrupt/old save) unit-tested. Save format: ChatSave { msgs, checkpoints }
with MsgSave { role:u8, variants, cur } — branches and the selected variant survive a restore.


## int8-resident weight cache (fast reload / crash recovery)
Q: keep things as GPU objects on UMA so a crash doesn't re-pull/re-deser from disk? A process
crash frees all GPU buffers (on UMA they're system RAM, reclaimed by the OS), so you can't keep
the literal buffers across a crash — but you CAN cache the ready int8 form so a reload skips the
expensive work.

Where the time goes: every load runs `gguf::dequantize` (Q8_0 → f32) then re-quantizes f32 → int8
on the CPU — a full f32 round-trip (a Q8 model balloons to ~13 GB of f32 and back). That dequant,
not conversation deser, is the real "pull from disk" cost (~10-12s of the 15s), repeated every
launch/recovery.

Fix: after the first load the ready int8 weights (QWeight: packed int8 `qw` + scales `qs`, plus the
f32 embed/norms) are written to `<model>.int8cache`, tagged with the GGUF's (size, mtime). Later
loads read it straight into GPU buffers via `load_int8_cache`, skipping the dequant. Gate "int8
cache reload == fresh load" is BIT-EXACT (max|d|=0.00e0); a stale fingerprint is rejected. The
engine stays dependency-pure (hand-rolled LE blob, bounds-checked reader). On UMA this is the
compact GPU object worth keeping resident; a page-cache-warm reload after a crash is an upload, not
a dequant.

Residual / follow-ons: (1) the GGUF is still fully read on a cache hit for the tokenizer — read
only the header prefix (metadata precedes tensor data) to drop that 3 GB read; (2) cache the
tokenizer too → zero GGUF touch on hit; (3) mmap the cache for true zero-copy on UMA; (4) the real
"keep resident across a UI crash" answer is a separate long-lived engine daemon the UI reconnects
to — biggest change, biggest payoff. Cache is ~GGUF-sized on disk (opt-out by deleting the file).


## Warm-resume auto-continue (no re-prefill) + chat export
Observed on M2 Max: auto-continue after a cap-hit re-prefilled the whole accumulated context
(1693 tokens → 27s) before adding more — the "had to warm up again". Cause: the continuation built
a fresh continue-prompt whose suffix (the 512-token partial reply) exceeded the reuse suffix cap,
so reuse was disabled and it full-prefilled. But the session that just produced those tokens
ALREADY held the entire context warm in its KV cache.

Fix — `Engine::continue_last` (warm resume): take the anchor session, copy its live KV into a
session with headroom via `seed_from_prefix` (a GPU buffer copy, NOT a forward pass), carry over its
`logits` (the live next-token distribution), and keep decoding. No prefill at all. Gate "warm-resume
continuation == straight-through decode" confirms the resumed tokens are IDENTICAL to decoding with
room from the start (bit-exact). This is the user's "hold, don't evict, come back to life": the
session is revived from its own KV instead of rebuilt from text.

Wiring: Backend trait gains `continue_last(...) -> Option<Result<usize,String>>` (default None →
caller falls back to a prompt generate; llama.cpp uses the fallback). New `Cmd::Continue(fallback)`
+ `Inference::continue_stream`; the worker tries warm-resume, falls back to `generate(fallback)` if
the backend returns None. Chat `continue_generation` now calls `continue_stream` (passing the old
continue-prompt only as fallback). Re-tokenization drift is also avoided — warm-resume continues the
ACTUAL token ids, not a re-tokenized detokenization of them.

Export: "⧉ Export" button (right of the checkpoint bar) → `transcript()` (Markdown: # logscope chat,
## You / ## Assistant) copied to clipboard AND written to ~/.local/share/logscope/exports/chat-<unix>.md,
with a toast confirming the path.

Still open (the proactive half of the ask): "step in when context is low / model about to die" beyond
the existing context-raise banner — e.g. auto-compact/summarize older turns when the window fills, or
warm-resume pre-emptively before a hard stop. Warm-resume removes the continuation COST; deciding WHEN
to continue/compact is the next layer.


## Continuous decode for auto-continue (no segmented re-prefill)
The single-instance half of "start the next early / run both and merge": rather than stop a reply at
`max_tokens` and resume, when auto-continue is on the engine now decodes the whole budget in ONE
continuous session. There is no mid-reply boundary to hide — it's removed, not overlapped (a plain
continuation can't be parallelized: token N+1 needs token N).

New `GenParams.reply_ceiling`: the per-reply continuous budget. Chat sets it to
`max_tokens * (1 + MAX_AUTO_CONTINUE)` when auto-continue is on, else `max_tokens`. `max_tokens` stays
the per-chunk / manual-continue size. Engine `generate` decodes up to `reply_ceiling`, stopping early
on eos.

Prompt-vs-reply budget fix: the old code reserved the full reply budget when fitting the prompt, so a
big continuous ceiling would over-truncate a long prompt. Now it reserves only a MINIMUM reply slice
(min(ceiling,256)) when capping the prompt, then sets `n_new = ceiling.min(window - prompt_len)` — the
reply budget yields to the prompt and fills whatever context room remains. llama.cpp path mirrors this
(decode to `reply_ceiling`, capped to `n_ctx - n_prompt` so it can't overrun the KV window).

Removed the segmented machinery (`continues_left`, `want_continue`, the apply_evt re-arm, the ui()
consume loop) — the continuous budget replaces the N× re-arm. Kept `hit_cap` + the manual "⏭ Continue"
button, which warm-resumes ONE more chunk (`continue_last` still uses `max_tokens`) when a reply runs
the full budget. Streaming status shows "chunk K/N" once a reply flows past the per-chunk size, so the
512 marks are visible as display boundaries without a decode stop.

Net: with auto-continue on, a long answer streams straight through to the ceiling at steady tok/s with
no 27s re-prefill gap and no visible stop; beyond the ceiling, manual Continue extends a chunk at a
time via warm resume. The parallel/speculative half (draft-ahead + verify) is the next visit.


## Throughput coordinator: A+B+D (memory-bounded batched decode)
Build order step 1 from THROUGHPUT_SCALING.md. Turned out A (tick-driven continuous batching) and B
(shared prefix prefilled once, KV-forked per branch) ALREADY existed in `WorkerPool` — it's stateful,
stepped a tick at a time, with park/revive/retire. The missing piece was D: admission was bounded by a
fixed `max_batch` COUNT, not by KV memory.

Added (D): `WorkerPool::with_kv_budget(bytes)` + `can_admit()`. Each session reserves
`kv_per_session = n_layers · kv_dim · 2 · 4 · max_seq` bytes of KV up front; `fill()` now admits only
while `prefix_kv + (active+warm+1)·kv_per_session ≤ budget`. A first-session guard means a too-tight
budget degenerates to sequential admission rather than deadlocking. `peak_active()` / `kv_per_session()`
expose the scaling readout.

Gated (bit-exact): KV-budget admission changes only WHEN a branch admits, never its tokens — a tight
budget serializes (peak==1) and a roomy one runs all concurrently (peak==3), and BOTH produce per-branch
output identical to independent `generate_kv` (max|d|=0.00e0). So the memory ceiling gates safely and
output-invariantly.

Measure it: `kernel-validate --coordinator <gguf> [n_req=32] [budget_mb=2048] [prefix_ctx=512] [max_new=64]`.
Reports KV/session, how many fit the budget, the peak concurrent batch actually reached, and aggregate
tok/s. Sweep `budget_mb` upward: peak concurrency rises with the budget until aggregate tok/s plateaus
(the compute knee) or it OOMs (the KV wall). This is the "how many instances" dial — but note instances
here are batched sessions on ONE resident model, which is the right way to scale on one GPU.

NOT done this turn (the app half of A): wiring this coordinator into the egui worker so the chat drives
concurrent requests/branches. The engine substrate + measurement are validated; the app integration is a
UX step (the chat is single-conversation today — concurrent sessions are the branch/voting feature).
Next per the plan: C (int8-KV sessions → ~4× the budget ceiling, via the existing run_batched_q8 path).


## int8-KV coordinator: ~4× the budget ceiling (plan item C)
The coordinator's ceiling is KV memory. int8 KV is ~¼ the bytes of f32 (packed int8 K/V = kv_dim
bytes/row vs 4·kv_dim, plus one f32 scale/row), so the SAME budget admits ~4× more sessions.

Rather than duplicate WorkerPool, it's now generic: `WorkerPool<'a, S: KvSession = Session>`. The
`KvSession` trait abstracts the two backings (f32 `Session`, int8 `SessionQ8`) — `kv_per_session`,
`make_prefix`/`make` (admission incl. prefix fork), `sample_next`, `decode_batch` — so admission, the
KV budget, continuous batching and park/revive are written once. The default `S = Session` keeps every
existing call site working (annotated `WorkerPool::<Session>::new` where inference needed help).

Added the int8 shared-prefix primitives so q8 gets B too: `prefill_prefix_q8` (prefill the shared
prefix once into an int8 cache) + `seed_from_prefix_q8` (KV-fork it per session, a buffer copy). So
q8 mode has full A+B+D parity with f32.

Gated (bit-exact): the q8 coordinator's per-branch output equals `generate_kv_q8` (max|d|=0.00e0) —
which also validates the new q8 prefix-fork — and KV/session is >=3.4× smaller (synthetic model with
kv_dim=32 measured 3.56×; real models at kv_dim 512–1024 approach 4× since the per-row scale overhead
shrinks). int8 KV is lossy vs f32 (it matches the int8 reference, not the f32 one), same precision as
the int8 weights already shipped.

Measure: `kernel-validate --coordinator <gguf> [n_req] [budget_mb] [prefix_ctx] [max_new] [q8]`. The
`q8` flag runs the int8-KV pool; compare its peak concurrent count to the f32 run at the SAME budget_mb
— it should be ~4×. The bench dispatches through one generic `run_coordinator::<S>` so f32 and q8 share
the body. Suggested first sweep: a fixed budget_mb, f32 vs q8, watch peak concurrency and aggregate
tok/s. Next per the plan: E (overlap prefill with decode so a big new prompt doesn't stall the batch),
then F (prefix trie), then the app-side wiring + speculative.


## Interleaved admission: overlap prefill with decode (plan item E)
Before: the coordinator prefilled a newly admitted request's whole suffix synchronously at admit time
(inside fill via `S::make`), so a big prompt froze the running batch for the full prefill. The egui-side
symptom was the 27s warm-up stalls.

Now admission is incremental. `make` split into `make_empty` (session + prefix fork, no suffix) +
`prefill_chunk` (prefill a block at the current pos). New pool state `admitting: Vec<(Branch, S, suffix,
cursor)>`. Per `step()`: `fill()` moves pending → admitting (budget/count bounded; admitting sessions
hold KV so they count toward `can_admit`); `advance_prefill()` spends up to ONE `PREFILL_CHUNK` (256) of
total prefill work across the admitting queue — small prompts finish within the chunk so several admit in
one tick, a big one takes one chunk/tick — and a completed session joins `active`; then the active batch
decodes one token as before. So the running stream keeps producing every tick instead of freezing; a long
prefill becomes a slowdown spread over ⌈suffix/256⌉ ticks.

Honest framing: one GPU can't truly decode and prefill at once — this INTERLEAVES (bounded prefill work
per tick) rather than parallelizes, converting a stall into graceful degradation. Output is unchanged:
chunked prefill is the same sequential decode_steps regrouped.

Gated (bit-exact): chunked-admission output == one-shot-prefill output (max|d|=0.00e0); a 600-token prompt
(>256) goes active at tick 3 (prefilled over 3 chunks); and a long-running branch emitted tokens during
that prefill (the overlap). `is_idle`/`retire`/`can_admit` updated to include the admitting queue.

Remaining plan: F (prefix trie via extend_prefix — shared sub-prefixes across branches, the branch/voting
substrate), then the app-side wiring of the coordinator, then speculative folded into the batch.


## Prefix trie (plan item F): already built — extended to int8 + made measurable
F's core already existed: `run_fleet_trie` builds a radix trie over all request token sequences,
prefills each shared node ONCE (`prefill_prefix` at roots, `extend_prefix` for child segments reusing
the parent cache), groups workers by their deepest shared node, and runs each group via `run_batched`
seeded from that node. Gated bit-exact: "fleet trie (nested prefix) == per-worker" — workers sharing a
path ([3,1,4] then [3,1,4,1,5] then diverging) decode identically to standalone. This is the
branch/voting substrate: forking a conversation = one shared prefill + cheap divergent tails.

Two gaps closed this turn:
- **int8 trie edge** — added `extend_prefix_q8` (+ `seed_from_prefix_q8` from the C work): the int8
  analogue of `extend_prefix` (copy parent's packed-int8 K/V rows, prefill the segment at the offset via
  decode_step_q8). Lets the prefix trie run on int8-KV, combining the trie's shared prefill with C's ~4×
  concurrency. Gated bit-exact: a node built by `extend_prefix_q8`, KV-forked into a q8 session and
  decoded, equals a from-scratch `generate_kv_q8` of (parent ++ segment) (max|d|=0.00e0).
- **measurement** — `kernel-validate --fleet <gguf> [branches] [shared] [tail] [max_new]`: N branches
  share a `shared`-token prefix + a unique `tail` each; A/Bs `run_fleet_trie` with reuse on (min_share=2)
  vs off (min_share=N+1). Reports prefill tokens saved (trie = shared + N·tail vs naive N·(shared+tail) →
  (N-1)·shared saved) and the wall speedup, and confirms outputs are identical (reuse is output-invariant).

Remaining: the trie is the one-shot `run_fleet_trie` path; folding dynamic trie reuse into the STATEFUL
coordinator (WorkerPool admits branches that longest-prefix-match cached nodes) is the integration step,
alongside wiring the coordinator into the app and the speculative fold. The pieces — generic pool,
budget, interleaved admission, f32+int8 prefix forking and trie edges — are all in place and gated.


## Dynamic trie reuse folded into the stateful coordinator (F → WorkerPool)
The trie lived only in one-shot `run_fleet_trie`; the stateful `WorkerPool` had a single global prefix.
Now folded in. The pool holds a `nodes: Vec<PrefixNode<S>>` trie of cached shared-prefix KV (each node:
tokens + cache + len), generic over f32/int8 via the new `KvSession::extend` (= `extend_prefix` /
`extend_prefix_q8`). New API:
- `cache_prefix(tokens) -> node_id`: cache a shared prefix, reusing the longest existing node that is a
  prefix of `tokens` (trie-extend — only the new segment is prefilled) or prefilling from scratch.
- `admit_at(id, label, node_id, suffix, max_new)`: fork a branch from a cached node, prefilling only its
  divergent tail. `admit(...)` still forks from the global prefix (node 0) for back-compat.

This is the branch/voting substrate IN the coordinator: cache the conversation once, fork N branches
from it (each KV-copies the shared node, prefills only its continuation), all decoded together at the
sub-linear batched rate, bounded by the KV budget, with int8-KV for ~4× the branches. Per-branch and
nested-node correctness is gated bit-exact: "stateful coordinator trie (cache_prefix + admit_at) ==
independent" — 4 branches forked from a cached prefix + a deeper extend-built node, every output equal to
standalone generate_kv (max|d|=0.00e0).

Budget note: nodes are independent caches (a child re-copies the parent's rows), so caching K overlapping
prefixes costs sum-of-lengths of KV, not unique tokens — the trie's win is the one-time PREFILL saving,
not KV dedup. For branch/voting (one cached conversation, many forks) that's a single node.

The coordinator stack A–F now lives entirely in one generic, tick-driven, budget-bounded, prefix-trie
WorkerPool — all gated. Remaining: wire it into the egui app (the branch/voting UI driving cache_prefix +
admit_at per fork), and fold speculative verify into the batched step.

---

## UI parity + native dialogs + chat focus fix (this segment)

Four app-side items, all compile-validated (the sandbox is headless, so portal/dialog *runtime* behavior
is M2-side; engine `gpu-gemm` untouched this segment).

**Chat composer focus bug — fixed.** Typing the first character dropped focus on the chat input. Cause: the
suggestion chips are rendered only while the input is empty (`if self.input.trim().is_empty()`) and sit
directly above the composer, so the first keystroke removes those widgets and shifts the composer
`TextEdit`'s *auto-generated* id — egui drops focus from a widget whose id changes between frames. Fix: pin a
stable id, `.id(egui::Id::new("logscope_chat_composer"))`, so focus survives the empty→non-empty layout
change. (chat.rs ~line 842.)

**Native file dialog by default.** `filepicker.rs` now opens the OS picker via `rfd` on the **XDG desktop
portal** backend (`default-features=false, features=["xdg-portal","async-std"]`) — no GTK, matching the
glow/no-GTK stance; ashpd/zbus are pure-Rust and build clean in the sandbox. The portal future is driven by
`async_std::task::block_on` on a **background thread**, with the result delivered over an mpsc channel so the
egui frame never blocks. It's baked *inside* `FilePicker` (new `native: Option<NativeJob>` field; `start()`
spawns the job when `native_available()`, `ui()` polls it before the modal), so **zero call-site churn** —
app.rs/sources.rs/chat.rs all keep using `open_file`/`open_dir`/`ui(ctx)→Outcome`. `native_available()`
auto-falls-back to the built-in std::fs modal when no `WAYLAND_DISPLAY`/`DISPLAY`/`DBUS_SESSION_BUS_ADDRESS`
is present (headless/SSH). A "native dialogs" checkbox by the models-folder row toggles it process-wide via
`set_native`/`native_enabled`. New deps: `rfd 0.15`, `async-std 1` (app only; engine stays dependency-minimal).

**Chat prose markdown — parity with GTK `ui/markdown.rs`.** The egui chat segmented Think/Code/Prose but
rendered Prose as one plain `ui.label`. Added a hand-rolled inline-markdown renderer (no new dep) covering the
same scope as the GTK view minus fenced code (already handled by `Seg::Code`): ATX headers (`#`..`######`),
`**bold**` (via the visuals' strong color — egui's default fonts have no bold *weight*), `*italic*`/`_italic_`,
`` `inline code` `` (monospace + cyan), bullet lists (`- `/`* `/`+ `), and `[text](url)` links as styled
(cyan, underlined) text. Inline runs are assembled into a wrapping `LayoutJob`; headers/bullets are
line-level. The streaming cursor (▌) lands on the last line. (chat.rs: `parse_inline` / `md_line_job` /
`render_markdown`, wired at the `Seg::Prose` arm.) Link *targets* are dropped (display text only) — egui
LayoutJob runs aren't individually clickable; full click-through would need per-link Hyperlink widgets.

**Tab-level parity** already holds: the egui tabs (bloc, cargo, charts, chat, code, modify, peek, rotate,
sources) cover the GTK tabs and add `charts`. Remaining within-tab gaps to assess later: the GTK `ollama.rs`
(an Ollama HTTP provider backend — would need an HTTP dep) and shared `components.rs` widgets.

**DeepSeek-style models — design note** (see outputs/DEEPSEEK_SUPPORT.md, analysis not implementation):
"DeepSeek" splits into (1) the R1 *distills*, which are dense Llama/Qwen — the Llama ones (e.g.
R1-Distill-Llama-8B) should run **today** on the llama path, and their `<think>` output already renders via
`Seg::Think`; and (2) **native V2/V3/R1**, which use **MLA** (latent-compressed KV cache) + **MoE** (routed
experts) — a large, separate engine project, with full V3 (671B) out of range for 32GB UMA regardless. The
high-leverage move is **Tier 1: add Qwen2 arch** (read `qwen2.*` metadata instead of assuming `llama.*`, add
the Q/K/V *bias* terms, parameterize RoPE θ) — everything else (GQA, SwiGLU, RMSNorm, int8, KV cache, the A–F
coordinator) is unchanged, it's bit-exact gateable, and it unlocks the whole R1-Distill-Qwen line plus
Qwen2.5. (The current "qwen2.5 fails: missing embedding_length" is exactly the `llama.`-prefixed metadata
lookup missing `qwen2.embedding_length`.)

### "System file picker": KDE/Dolphin + COSMIC + portal routing

Generalized the native dialog into a **desktop-aware resolver** (`resolve_system_picker` in filepicker.rs).
The XDG portal is the standard route — `xdg-desktop-portal-kde` renders the KDE (Dolphin-family) dialog,
`-cosmic` the COSMIC picker, `-gnome`/`-gtk` the GTK one — so rfd's portal backend already reaches all of
them when the matching backend is installed. On top of that:
- **KDE/Plasma** (`XDG_CURRENT_DESKTOP` contains kde/plasma) → prefer the **`kdialog`** binary directly: the
  genuine KDE (Dolphin-style) file dialog, and robust even when xdg-desktop-portal-kde isn't installed;
  falls back to the portal (kde backend) if kdialog is absent.
- **COSMIC** (contains cosmic) → **portal** (its native picker is reachable only via xdg-desktop-portal-cosmic;
  no CLI dialog exists), else built-in.
- **GNOME / other** → portal (gnome/gtk backend) → `zenity` → `kdialog` if present → built-in modal.
- **Headless/SSH** (no WAYLAND_DISPLAY/DISPLAY/DBUS) → built-in std::fs modal.

CLI dialogs (`kdialog`/`zenity`) run via `std::process::Command` on the existing background thread and parse
the chosen path from stdout (exit≠0 = cancel) — **no new dependency** (portal still uses rfd+async-std from
the prior segment). The picker runs off the UI thread and delivers over the same mpsc channel, so the frame
never blocks. `system_picker_label()` reports the resolved backend ("KDE / Dolphin (kdialog)",
"COSMIC (portal)", "GNOME (portal)", "zenity", "built-in") and is shown in the settings tooltip; the
checkbox (relabeled "system file picker") toggles system-vs-built-in process-wide. Compile-validated only —
which concrete dialog appears is M2/desktop-side. Note for the user's setup: on KDE install `kdialog` (ships
with Plasma) or `xdg-desktop-portal-kde`; on COSMIC install `xdg-desktop-portal-cosmic`.

### Auto-merge of stop/start file fragments (continuity + model handoff)

When a reply hits the cap and is resumed (warm-resume "Continue", which streams into the *same* message,
`idx: last`), a file the model was writing gets split: it reopens a ```lang fence mid-file, and the old
parser gave each anonymous block a unique name (`file1.rs`, `file2.rs`) → "Generated files (2)" of one
logical file. `workspace::parse_artifacts` now **stitches** these back into one whole file:
- The fence scan breaks on a *new* opening fence (not just an exact ``` close), so a cut-open block + its
  reopened continuation become two adjacent blocks rather than one with a stray fence line.
- Path resolution gained a continuation rule: an anonymous block whose language extension matches the
  *previous* file's extension inherits that file's path (so a named `main.rs` followed by an anonymous
  `rust` continuation both resolve to `main.rs`). Anonymous blocks otherwise get a stable per-language
  name (`snippet.<ext>`) so repeated anonymous same-language pieces coalesce too.
- Same-path blocks are **coalesced**: content appended in order, `Artifact.fragments` counts the pieces.
- Distinct *named* files stay separate (explicit path always wins over the continuation rule).

This is done in the derived artifact layer only — the raw transcript/message text is left verbatim (safer
for replay/threading). The whole-file artifacts flow straight into save/zip/save-to-dir, so the **backend**
writes complete files (continuity), and handing the workspace to another model gives it whole files, not
fragments (handoff). **UI** reflects it: the header reads "Generated files (N) — k stitched from stop/start"
and each reassembled row shows a "merged ×k" badge with a hover explaining it. Covered by parser unit tests
(reopened-fence stitch, named+anonymous continuation join, distinct-files-stay-separate, anonymous naming) —
all 7 workspace tests pass. App `Artifact` gained a `pub fragments: usize` field (only construction site is
the parser). No new deps; engine untouched.

### Project-history vault: content-addressed branching tree (Phase 1)

Decided against git as the backend (see HISTORY_DESIGN.md): logscope ingests multi-GB logs, a safety
mechanism mustn't be able to fail, and git tracks *diffs* while we need *provenance* — this file became that
file *because* of the reasoning in turn N. New module `vault.rs` (app-side; zero new deps — `twox-hash`,
`serde`, `serde_json` already in tree; engine untouched).

Model: generated files are a deterministic function of the transcript, so we version the (small, text)
transcript and re-derive files. A **node** = a commit = `{id, parent, children, label, cum_len,
added:[MsgRef], files:[FileEntry], inputs:[InputRef], note}`. `added` is the message *delta vs parent*, so a
child shares the parent's message blobs (prefix sharing — the same shape as the KV prefix trie). Everything
is content-addressed by a 128-bit id (two seeded xxhash passes; blake3 is the export-grade upgrade). Each
`FileEntry` carries `produced_by` = the blob hash of the message that generated it, so "why is this file like
this?" resolves to that turn's full text; `file_history(node, path)` walks root→node and returns the ordered
*changed* versions, each linked to its producing turn — the provenance chain git can't express. External
inputs (logs/attachments) are recorded **by reference** (`{path,size,hash,mtime}`) and only copied under an
8 MB threshold, hashed in 1 MB chunks so a 1 GB log is identified, never loaded — this is the 1 GB cliff
handled. On disk under `~/.local/share/logscope/vault/`: `blobs/<2>/<hash>`, `nodes/<id>.json`,
`index.json`; atomic writes (tmp+rename); individual blob files → seekable, concurrent reads, no giant file
to parse.

7 unit tests pass: blob dedup+roundtrip, commit-chain delta + transcript, fork shares prefix blobs,
file-history links versions to reasoning (and doesn't duplicate an unchanged version), large input referenced
not copied, streaming hash == in-memory hash, reopen persists head+tree.

Wiring (compile-validated; runtime is M2-side): `Chat` gained `vault/head/show_history/file_hist`;
`vault_commit()` records every completed turn (auto-tracking) with files linked to the producing assistant
turn; `return_to_node()` snapshots the current line, loads a node's transcript, and points head there so the
next turn forks. UI: a "🌲 Tree" panel lists nodes (id ←parent, label, msgs, files) with **Return**, and each
Generated-files row gets a "↟" provenance toggle showing prior versions + the reasoning snippet behind each.
`head` starts None each session (new root; prior trees persist and are reached via Return — safe vs a
mismatched head). Future API (set_head/roots/branches/set_branch/children/has_blob/with_inline_max) is marked
`#[allow(dead_code)]` for Phases 2–3 (parallel branches via the coordinator; persisted int8-KV warm-start;
git+LFS export). Limitation: editing a message inside an already-committed prefix isn't re-tracked yet.

### Graded conversation harness (--simconvo)

New `gpu-gemm/src/simconvo.rs` + a `--simconvo` mode on `kernel-validate`. Replays a built-in suite of
10 authored conversations (difficulty 1→5: tiny code, short factual, multi-part code, bug-fix, multi-turn
context carry, reason-over-embedded-file, two-file cross-file refactor, **continue-mid-file** (the case that
broke), long synthesis, and an underspecified+long degeneration probe). Authored, not model-generated — a
135M model writes mush (that's what we're measuring), and the difficulty is graded by content + context
length; the hardest convos embed a real ~70-line Rust file so the context is genuine, not filler.

Per conversation it records: working-ctx tokens, prefill ms (+ the same resource-setup / gpu-sync split as
--bench), decode tok/s, output tokens, stop reason (eos/cap), and a **degeneration score** (4-gram repeat
ratio). Decoding is GREEDY (argmax, no repeat penalty) on purpose — a penalty would hide the looping the
degen score is meant to catch. Writes a Markdown report (`<gguf>.simconvo.md`) to paste back. Zero new deps
(suite embedded as Rust data; gpu-gemm stays wgpu/pollster/bytemuck only). Compile-validated here; real
numbers are M2-side — run `kernel-validate --simconvo <gguf> [out.md] [max_new=96]`.

Bonus: `build_prompt` detects the chat-template family from the tokenizer's special tokens (llama3
`<|start_header_id|>` vs ChatML `<|im_start|>` vs plain) and formats with the *correct* one — the same fix
needed for the model-handoff leak (each model gets its own template, not another model's markers).

Bench read (from M2 data, smollm2-135m): decode ≈ **22 ms dispatch floor + ~22 µs × ctx** (gpu-sync linear
in ctx: 32.8/65.9/87.1 ms at ctx 512/2048/3000). Decode usable to ~3k; the 5888 hang is the *prefill*
cliff, not decode. ⇒ two separable levers: lower the floor (fuse dispatches + m=1 mat-vec + cache bind
groups), and keep working-ctx ≤3k (the FSM budget) + reuse cached KV to avoid the prefill cliff.

### AST / sub-file reasoning (probed, not yet built)
tree-sitter + tree-sitter-rust **do build in this sandbox** (C grammar compiles via cc), so the Rust-AST
sub-file extractor is viable to add (in the app/lib, never the engine). Plan: parse generated Rust → items
(fn/struct/impl/enum with name+span) as sub-file units, tag each with (file, vault-version, task), and make
the reasoning-comparison axes queryable: file a vs b (different targets), a-v1 vs a-v2 (same file evolving),
task A vs B. Scoped as its own focused build.

### Editable conversation suite + Rust AST sub-file extractor

**Suite is now data, not just source.** `kernel-validate --simconvo-dump [file.txt]` writes the 10 built-in
convos in an editable plain format (`# id | diff | stress`, then U:/A: turns, multi-line until the next
marker; `%FILE%` expands to a sample Rust file). `--simconvo <gguf> <file.txt>` runs an edited file; with no
file it runs the built-in suite. The dumped suite ships at logscope-suite/simconvo_suite.txt. Harness
refactored to an owned-convo model (builtin() converts the static SUITE; parse_suite/dump_suite handle the
text format). Still zero new deps in gpu-gemm.

**Rust AST sub-file extractor** (`logscope-egui/src/ast.rs`, app-side — never the engine). tree-sitter +
tree-sitter-rust (the C grammar compiles via cc; confirmed building in the egui app, +~1m). `extract(src) ->
Vec<Item>` pulls fn/struct/enum/trait/impl/mod/const/static/type/union/macro with name, parent (impl type
or enclosing mod), and line+byte span. Methods inside impls carry the impl type as parent; items inside mods
carry the mod. Logic unit-tested in isolation (extracts names+parents correctly; a-v1 vs a-v2 detectable by
hashing the item's byte span) — those two tests pass; the app integration is compile-validated. New deps
(app only): tree-sitter 0.24, tree-sitter-rust 0.23. Module is `#![allow(dead_code)]` until wired.

This is the foundation for the reasoning-dimension comparisons: the sub-file Items are the units; the caller
hashes their spans with the vault's hasher to detect change, and the three axes — file a vs b (different
targets), a-v1 vs a-v2 (same item evolving, same name + different span-hash), task A vs B — become queries
over (Item × vault node × file). The metric itself is the next design+build; the extractor + version-change
detection are in place and validated.

- simconvo suite now 13 convos: +bigfile-1k/2k/3k (embed BIG world.rs + BIG2 mesh.rs modules, climb ~1k/1.8k/2.4k ctx) to exercise the prefill cliff, not just content difficulty. %BIG%/%BIG2% expand in suite txt.

- OOM fix: simconvo routes prefill through prefill_chunked(512) (memory-bounded, O(chunk·ff) transients) instead of prefill_batched (O(T·ff) → OOM at ctx~2k on 8B). `--gate-prefill` proves bit-exact for T<=chunk, argmax-identical above (worst |Δlogit|=1.4e-5). Cliff *speed* (fused QKV/decode-block dispatch) still pending — needs column-split kernel + per-path gates + M2 timing.

- Quality axis (part 1 of metrics): simconvo now logs a judge-free `qual` column = rubric facts a correct answer must name (rubric_for/score_rubric, `|`=alternatives). Foundation for model-pairing + a logscope metrics page; same rubric scores human transcripts. `--gate-rubric` self-tests the scorer (partial credit: wrong-method/right-symptom = 1/2).

- Branch/K experiment (`--simbranch <gguf> [suite] [K=5] [temp=0.8] [max_new=64]`): forks K stochastic continuations per rubric-scored convo, measures fact_divergence (branch disagreement on the known facts), best-of-K & consensus reconciliation, and correlates divergence with where greedy missed facts (Thesis A: does best-of-K beat greedy; Thesis B: does divergence predict failure = where to fork/steer). `--gate-branch` self-tests the math (no GPU). Numbers are M2-side.

- Typed tree vertices (logscope-egui/src/vertices.rs, mod vertices): segment(text)→Vec<Vertex> with kinds ThinkStart/ThinkEnd/FenceStart/FenceEnd (LITERAL, byte-exact merge) + Step/Code (REASONING; Code via ast.rs tree-sitter). merge_points(a,b) = monotone alignment to shared vertices = the branch merge sites (and ThinkStart = the flag to let a fast model run ahead during a reasoner^as think block). Literal/step/merge logic unit-tested standalone; full segment() (with ast) compiles in the app. Foundation for #1 speculative-merge + the tree diff view.

- LAN fabric mock (logscope-egui/src/fabric.rs, mod fabric): Worker trait (prefix TOKENS + model + sampler -> tokens; Local/Remote share it), Registry with per-model public/private + available_models() (dropdown source), dispatch() routes to a public-serving box, fan_out() = one provenance-tagged node per box (throughput play). FabricNode records WorkerId{device,author}+model = tree provenance. gate_fabric() self-tests 6 invariants (no net). RemoteWorker(HTTP)+certs = app/your-LAN step.
- Redline diff (logscope-egui/src/diff.rs, mod diff): word-level LCS -> Equal/Del/Ins ops + redline() (~~del~~ {+ins+}) + change_ratio() branch-distance. The track-changes engine for the tree highlight-changes view; egui strikethrough/color render is app-side. gate_diff() self-tests.

- LAN fabric net (logscope-egui/src/fabric_net.rs, mod fabric_net): (1) entry-point scheduler — SizeClass::from_params (Tiny<.5B/Small<2B/Mid<5B/Large<10B/XL), EntryPoint{class,load,est_ms_per_tok,public}, per-system Policy{min_class,w_fit/load/latency}, rank_entries/best_entry = "smallest-sufficient, idle, fast" optimal entry point. (2) hand-rolled length-prefixed binary wire (encode/decode_request+result; no serde) carrying prefix tokens + sampler + files. (3) tagged compression Codec{Raw,Rle} for file payloads (best_codec picks smaller; Deflate/Zstd = 1-fn swap, deferred: disk). (4) Transport trait + MockTransport (loopback) + RemoteWorker<T> impl Worker = the Remote half behind a mock; real HTTPS transport = your boxes. gate_net() self-tests all 6 (ran 1032→161B compress). ModelInfo gained params_b.

- Reverse-anchored context graph (logscope-egui/src/context_graph.rs, mod context_graph): insert(context,endpoint) anchors endpoint then inserts context REVERSED (nearest-endpoint first) so shared backward suffixes merge; minimal_context(context,endpoint) walks back until the path is unique (hits==1) = fewest tokens that determine the continuation = the "keep context down" lever (sideband retrieval; model stays FORWARD, not run backward). Literal/exact layer (semantic = embeddings). gate_context_graph: divergent endpoint needs 1/3 tokens (3x cut), nodes 12<17 naive. DAWG-minimize = further size cut on huge workloads (noted).
- HTTPS transport (fabric_net.rs): http_post<S:Read+Write>(binary frame as POST body) + Connect trait + HttpTransport<C> impl Transport, mock-validated (POST framing well-formed; HttpTransport decodes canned result). TLS connect (rustls TcpStream) = feature-gated on your boxes, the only un-tested piece. gate_http passes.

- Associative bridge (context_graph.rs): associations() = endpoint->[(endpoint,weight)] by shared immediate-predecessor tokens (small-world layer BESIDE the literal index). bridge(adj,a,b,max_hops,min_weight) = richest/widest path (bottleneck-greedy, hop-capped) routing two unconnected fragments through the graph; max_hops+min_weight = per-model capability knob (strong model = fewer/stronger hops). gate_bridge: A–B indirect, routes A→C→B, 1-hop rejects, weight-floor prunes. Reframe: degrees of SEPARATION not freedom; bridging needs the associative layer, not the reverse-index.

- Asymmetric retrieval objective (logscope-egui/src/retrieval.rs, mod retrieval): Recall lattice Miss<Partial<Hit. HARD: regressed_pinned_hit() forbids any pinned (exact-layer-confirmed) hit dropping below Hit — pinned hits un-swappable so no-regression is REAL. SOFT: accept_update() iff net_movement>0 (more miss→partial than partial→miss). frontier() = non-pinned-hit indices the fuzzy small model may touch (can only help). within_noise_budget() = 5% global-compression bar. Strictly > summarization (which accepts hit loss). gate_retrieval passes 6.

- End-to-end pipeline (logscope-egui/src/pipeline.rs, mod pipeline): assemble(graph,query,endpoint,budget) = minimal prefix (context_graph) → retrieval gate (accept_update) → entry_points+best_entry schedule (fabric_net) → reg.dispatch (fabric) → provenance node. dispatch_with_context() runs the whole chain. gate_pipeline() validates the SEAMS across all 5 retrieval/fabric modules: 6→2 context cut carried through, Tiny excluded for a Small task, provenance "llama-1b · dustin@studio-m2" tagged. Registry gained public_models(). First cross-module integration; primitives compose.

- Runaway guard (logscope-egui/src/genguard.rs, mod genguard): runaway_stop(recent_text,thresh) stops a degenerating decode loop on (a) pseudo-template-markers emitted as TEXT — has_pseudo_marker normalizes garbled <||eot-id||>/<|(start - header|> variants — or (b) word_4gram_repeat >= thresh. Cuts the no-EOS runaway (the 1134-tok/87.9s smollm2-360m loop). Validated on the actual leaked transcript. WIRE INTO app decode loop. Root cause = app prompt builder not using harness build_prompt ChatML detection → model never emits real eos(2).

- Stop-set resolution (genguard.rs): stop_token_set(declared_eos, specials) = the GGUF eos_token_id + ALL known stop specials, break on the SET. ROOT CAUSE of DeepSeek-8B loop: app resolved eos=128009 but R1-Distill-Llama stops on 128001 (harness read tokenizer.ggml.eos_token_id=128001 and terminated fine); single-id watch missed it. Fix: app must read GGUF eos_token_id + stop on the set, not one id. gate_stop validates.

## BATCH 0 wired (correctness/looping) — 2026-06-12
- **eos resolution fixed** (backend.rs): GGUF `tokenizer.ggml.eos_token_id` is now the PRIMARY stop
  (was: `<|eot_id|>` first → picked 128009 for DeepSeek-R1-Distill, whose real stop is 128001 →
  whole-answer loop). Engine now also keeps `stop_ids` = the full EOG set and the decode callbacks
  halt on ANY of them (covers eot/eom/end_of_text/im_end/end-of-sentence). Confirmed against
  llama.cpp's EOG list {128001,128008,128009}.
- **template family detection** (backend.rs → Evt::Loaded(LoadInfo) → chat.rs): Engine detects
  ChatML vs Llama3 vs Plain from tokenizer specials; chat.rs builds the MATCHING prompt. Fixes the
  smollm2 loop (ChatML model was fed Llama-3 markers → byte-level marker text, never hit `<|im_end|>`).
  Renamed the misnamed `build_prompt_chatml` (actually Llama-3) → `build_prompt_llama3`; added a real
  ChatML builder; both `build_prompt_upto` and `build_continue_prompt` branch on `self.template`.
- **runaway guard wired** (backend.rs): both decode callbacks run `genguard::runaway_stop` on a rolling
  1200-char tail (0.5 ratio) → cuts degenerate loops that slip past the stop set.
- **repeat_penalty**: already 1.15 default (GenParams) and already threaded into the wgpu SamplerParams.
- **llama.cpp BackendAlreadyInitialized fixed** (llm_backend.rs): `LlamaBackend::init()` is now a
  once-per-thread shared Arc (`shared_backend()`), so loading a 2nd model no longer fails. (llama-ffi
  path; validates on M2 — not compiled in the sandbox.)

- simconvo suite +11 non-code convos (lyric/narrative reasoning: meaning-preservation, metaphor-consistency, contradiction-detection, song-arc). New %LYRICS% expansion + a LYRICS const. Rubrics are deliberately blunt "load-bearing anchors survived" proxies — creative tasks resist substring scoring, and that drift IS the measurement. gate-rubric extended (metaphor 4/4 vs 0/4, rhyme-vs-meaning 2/2). Suite now 26 convos.

## BATCH 1-3 (storage/memory/profiling) — 2026-06-12
- **BATCH 1 storage (done):** new gpu-gemm `store.rs` — content-addressed (`content_key` = len +
  sampled 64KiB head/mid/tail, std SipHash), CENTRAL cache dir (OS default: Linux
  `~/.local/share/logscope/int8cache`, macOS `~/Library/Application Support/...`; `LOGSCOPE_CACHE_DIR`
  override), `evict_to_cap` (LRU by mtime + sweeps `.tmp` orphans; `LOGSCOPE_CACHE_MAX_GB`, default 10).
  backend.rs now keys the int8 blob by content (fp.1 = content key, not mtime) → same GGUF across
  projects dedups to ONE blob, no more sidecars piling up next to models. `models_root()` resolver
  (+ `~/.ollama/models` auto-detect) ready for the unified models folder. gate_store: 11/11 (`--gate-store`).
- **BATCH 2 memory (slice + design):** load now logs the int8 resident footprint (≈ GiB) so memory
  pressure is visible before loading a bigger model. NOT yet done (M2/dep-gated, and 30B won't fit
  32GB as int8): the q4-resident path (dequant-per-op, ~17GB for 30B) + mmap GGUF — that's the real
  big-model enabler, deferred until there's space to measure it.
- **BATCH 3 perf (instrumentation done; fusion designed):** the engine already had GPU_SUBMITS/
  GPU_POLLS/GPU_SETUP_NS/GPU_SYNC_NS; backend.rs now prints a per-generate profile line —
  "N submits (X/tok), polls | setup Xms vs sync Yms (dispatch|GPU-bound)" — so a REAL M2 chat run
  answers "why is llama.cpp faster" (expect dispatch-bound: high submits/tok, setup≳sync). The fix
  (fused QKV mirroring the existing gate+up fusion, across ~10 call sites + build wqkv at load from
  wq/wk/wv concat so NO cache-format/disk change) is designed but deferred until the profile numbers
  justify the surgery and can be measured.

## BATCH 1 wired (storage unification) — 2026-06-12
- **int8 derived cache** (gpu_kernels::store, was already built + wired into Engine::load): content-
  keyed central blob in one shared dir, regenerated on miss, LRU-evicted under LOGSCOPE_CACHE_MAX_GB
  (10 GiB default) with `.tmp` orphan sweep. Same GGUF across projects dedups to one cache entry.
  OS defaults: Linux `~/.local/share/logscope/{models,int8cache}` (XDG), macOS `~/Library/Application
  Support/...`; LOGSCOPE_CACHE_DIR / LOGSCOPE_MODELS_DIR override; `~/.ollama/models` auto-detected.
- **model-level unification (NEW this pass)**: `content_key_file` (cheap seeked key, PROVEN equal to
  the full-bytes `content_key` in gate_store — so load-time and listing-time keys agree), `list_models`
  /`list_models_in` (dedup identical GGUFs across project folders → one entry), `duplicate_models`
  (report redundant copies + reclaimable bytes), `resolve_model` (reference a model by file name, no
  absolute path). Consumer: `kernel-validate --models` lists the unified folder + reclaimable dupes.
  gate_store extended 11 → 16 checks (file/in-mem key equality, dedup, duplicate detection, resolve).
- Still open in Batch 1: physical dedup (hardlink/symlink the redundant blobs — reported, not yet
  collapsed), full ollama blob/manifest resolution (Batch 4), and app-side model dropdown via
  resolve_model/list_models (Batch 6 UI). GGUF blobs are ~incompressible → win is dedup, not zip.

## BATCH 1 follow-up + OOM diagnosis — 2026-06-12 (M2 feedback)
- **ollama discovery** (store.rs): the user's whole library lives in ollama (sha256-* blobs under
  manifests, no .gguf ext → --models found nothing). Added list_ollama (parse manifests/<host>/<ns>/
  <repo>/<tag> JSON → .model layer digest → blobs/sha256-<hex>), names match `ollama ls`
  (repo:tag, library dropped). list_models_in routes an ollama-layout root (has manifests/) through
  it; resolve_model matches repo:tag too. gate_store now 20 checks (incl. ollama). --models prints names.
- Fixed unused-mut warnings (simconvo pick closure; store add closure).
- **OOM (Batch 2, not yet fixed)**: 70B (40 GB) is `std::fs::read` into a RAM Vec → instant kill
  before dequant. 8B kill is a Batch-1 REGRESSION: relocating the int8 cache + changing fp from
  mtime→content-key orphaned the old sibling cache, forcing a fresh dequant whose peak (4.7 GB held
  bytes + 2.1 GB resident f32 embed + accumulating int8 + Vulkan staging) tips 32 GB. FIX = mmap the
  GGUF (don't RAM-load it) + keep the embedding/big models q4-resident not int8. mmap alone recovers
  the 8B and the read-OOM; 30B needs q4-resident (int8 30B ≈ 30 GB won't fit); 70B is out on 32 GB.

## BATCH 2 part 1: mmap (autonomous session) — 2026-06-12
- **mmap the GGUF** (gpu_kernels::mmap_ro, memmap2 — a scoped model-loading infra dep, not serde/net).
  Engine::load + all bin loaders (simconvo, bench, qcheck) now mmap instead of fs::read. Fixes BOTH
  OOMs: the 70B (40 GB) no longer RAM-loads before dequant, and the 8B's dequant peak drops by the
  full file size (the held bytes are now OS-paged), undoing the Batch-1 regression. A cache HIT now
  faults only the content-key sample (~192 KB), not the whole file. `--gate-mmap`: mmap-load vs
  read-load is BIT-EXACT (0.000e0, argmax + content_key match) on lavapipe; full kernel suite still ✓.
- Still need for 30B: q4-resident (int8-30B ≈ 30 GB won't fit 32 GB w/ KV+compute). mmap fixes the
  read-OOM + recovers ≤~20B-as-int8; 30B needs the resident format change (deferred — needs real-model
  bit-exact validation the sandbox can't do; risky to ship blind). 70B is out on 32 GB regardless.

## BATCH 4: LocalWorker (autonomous session) — 2026-06-12
- **Gpu::run_tokens** (lib.rs): token-level generation — prefill prefix, decode max_new with
  sampler+seed, halt on a stop set; `&self` + local cache so one resident model serves many prefixes.
  `--gate-worker`: seed-deterministic, max_new hard-bounded, stop id halts before emitting it ✓ (lavapipe).
- **Engine::run_tokens** + **LocalWorker** (backend.rs, cfg wgpu-engine): wraps the engine in the
  fabric `Worker` trait so the local box is just another Registry node (same interface as a Remote box
  over the wire). generate() = tokens-in/tokens-out for its model; returns None for other models.
  Compiles clean with --features wgpu-engine. Now fan-out / best-of-K / branch dispatch run against
  REAL generation, not MockWorker. (Owns its own Engine for now; sharing one device across the chat
  backend + fabric is a later optimization.)

## BATCH 2 part 2: pre-flight memory guard (autonomous session) — 2026-06-12
- **No more OOM-kills.** Engine::load now estimates the int8-resident footprint from GGUF metadata
  (store::estimate_int8_resident / resident_bytes_for) and compares to available memory
  (/proc/meminfo MemAvailable, ×0.9, or LOGSCOPE_MAX_RESIDENT_GB). If it won't fit, it RETURNS an
  error ("won't fit: ~X GiB needed, ~Y available … q4-resident on the roadmap") instead of getting
  SIGKILLed mid-dequant. LOGSCOPE_FORCE_LOAD=1 bypasses. On the 32 GB M2: 8B (~10 GiB) loads, 30B
  (~36 GiB) + 70B (~84 GiB) refuse cleanly. gate-store now 24 checks (estimate ~10 GiB for 8B,
  monotonic, budget override, meminfo readable).
- NOTE for M2: with mmap + this guard, the 8B should now load + run on wgpu (the Batch-1 regression is
  gone). 30B still needs q4-resident (the real fix); until then it refuses rather than crashes.

## UI scaffolding (autonomous session, pure logic validated; egui render is the wiring step) — 2026-06-12
- **scroll.rs** — Firefox-style scroll as pure physics: exponential-decaying sensitivity boost (fresh
  gesture flings, sustained scrolling settles to 1:1 precision) + momentum friction. 6 unit tests
  incl. the spec'd content↔finger **R² ≥ 0.9**, friction settles to exact rest, idle gap re-boosts,
  monotone (no jank), flick covers more than dribble. Wire: feed raw_scroll_delta → input(); tick(dt)
  each frame → apply delta to ScrollArea. Tunables all public for live feel.
- **cards.rs** — ambient suggestion queue (Batch 9 + Batch 6). Key semantic: nothing injects into a
  worker until `accept`; `dismiss` suppresses re-nagging by content key; `defer` sinks priority;
  dedup by key, score-ordered top(). 4 tests. Kinds: LocThreshold / Discovery / Conflict / MergeEdit.
- **genviz.rs** — multi-stream token-gen state: per-worker smoothed tok/s (EMA), STABLE display slots
  (rows don't reshuffle), state machine (Prefilling→Decoding→Stopped), + `approach()` frame-rate-
  independent glide for fluid bars. 4 tests (transitions, slot stability, EMA damps bursts, approach
  converges + dt-correct).
- **palette.rs** — 4 color schemes (Midnight/Nord/Gruvbox/Paper) as role-based RGB data + WCAG
  contrast; 4 tests GUARANTEE every shipped scheme is legible (ink/bg ≥ 4.5 AA, dim ≥ 3.0). Live app
  still uses theme:: consts; this is the migration target for runtime scheme switching.
- All four declared in main.rs (#[allow(dead_code)]) so they compile into the app; app cargo check ✓
  with --features wgpu-engine. Remaining for each: the egui render layer (needs the running app to tune).

## BATCH 5: corpus ingestion (autonomous session) — 2026-06-12
- **corpus.rs** — populate the reverse-anchored context graph from REAL text (word-tokenized, sliding
  window: preceding ≤W words → this word). Gives the retrieval primitives an actual corpus instead of
  synthetic ids — the precondition for Batch 9 attention-graph scheduling. `min_context(phrase)` =
  fewest preceding words that uniquely determine the continuation on the real distribution.
  5 unit tests (validated on a real paragraph, not synthetic): distinctive bigram uniquifies in ≤1
  token; ambiguous continuation needs ≥ that; frequent endpoint ('the') has higher load than rare
  ('fox'); OOV query → None; and real node sharing vs the zero-sharing baseline (compression metric
  corrected — endpoint anchors dedup by word + shared suffixes merge, ~modest for window=4 prose).
  Declared #[allow(dead_code)] in main.rs; app cargo check ✓ with --features wgpu-engine.
- Honest note: the reverse-anchored index's big compression wins need repeated suffixes / large
  corpora; small diverse prose at window=4 shares modestly (each 4-gram nearly unique). The retrieval
  *behavior* (minimal-context, load) is the validated value here, not dramatic compression.

## BATCH 7 (partial): architecture guard (autonomous session) — 2026-06-12
- **Non-llama arches refuse cleanly** (lib.rs load_gguf_opt): reads general.architecture; anything but
  "llama" returns "architecture 'X' isn't supported by the wgpu engine yet … use the llama.cpp
  backend" instead of the cryptic "missing embedding_length" panic. Fixes the UX for the user's
  qwen3.5/gemma/phi3 ollama models on the wgpu backend. `--gate-arch`: llama loads, patched-to-qwen2
  refuses with guidance ✓. Full Qwen2 support (QKV bias + RoPE θ + broadcast-add kernel) deferred —
  validatable on synthetic but needs a real qwen GGUF to confirm tensor-name/bias assumptions; won't
  ship a blind hot-path arch change unsupervised.

## M2 feedback: ollama path fix — 2026-06-12
- M2 confirmed: gate-store 16/16 on hardware; full kernel suite ALL VALIDATED on the real Apple M2
  Max (user ran --gate-map, a typo that fell through to the default suite — every kernel green on
  actual hardware, not just lavapipe). Real decode profile (tiny synth model): wall 8.53ms =
  setup 0.30 + gpu-sync 7.17 + other 1.06, 2 submits/token → gpu-sync dominates, confirming the
  fewer-dispatches/fusion lever (perf, deferred).
- BUG FIXED: --models found nothing because the user's ollama runs as a service with models NOT at
  ~/.ollama/models. store::models_root no longer special-cases ollama; new ollama_roots() probes
  $OLLAMA_MODELS + ~/.ollama/models + /usr/share/ollama + /var/lib/ollama + /usr/local/share/ollama
  (qualifying only dirs with manifests/), and all_model_roots() = logscope dir + every ollama store.
  --models scans all + prints each ollama dir; resolve_model checks all ollama roots. gate-store now
  20 checks (ollama_roots/all_model_roots detection). If a custom ollama path, set OLLAMA_MODELS.

## M2 feedback pass — 2026-06-12
- M2 run confirmed: --gate-store 16/16 on real hardware ✓, and the FULL kernel-validation suite
  passes on the actual Apple M2 Max [Vulkan]/Honeykrisp GPU (every kernel OK, int8-resident argmax
  matches f32) — lavapipe correctness transfers to real silicon.
- --models found nothing in the user's log because that build PREDATED the all_model_roots wiring.
  Verified the current code auto-detects ollama: with HOME=~ and no env var it lists ollama models
  by repo:tag from ~/.ollama/models (+ probes /usr/share/ollama, /var/lib/ollama, $OLLAMA_MODELS).
  Rebuilding from the latest zip surfaces the 7 ollama models. (ollama_roots + all_model_roots.)
- Added: an unrecognized --gate-* now lists the available gates instead of silently running the
  default suite (the user typed --gate-map for --gate-mmap and got the whole suite). --gate-mmap etc.
  unchanged.
- M2 decode profile (synthetic 4-layer model): wall 8.53ms/token, 2 submits/token, gpu-sync 7.17ms
  dominates → confirms the dispatch-fusion direction (Batch 3) for the real speedup. (Synthetic, not
  a real 8B; real-model profiling still pending.)

## BATCH 6: merge-as-commit stitch (autonomous session) — 2026-06-12
- **merge.rs** — applies a lazy partial edit (the `// ... existing code ...` / `// file:` convention)
  onto a full original file, item-by-item: a redefined top-level item REPLACES its same-named
  original; a new item is appended; an `impl T` block merges inner fns (same fn replaced, new appended,
  siblings kept); unmentioned code untouched. Pure brace/line scanner (no tree-sitter dep, scratch-
  testable). `summary()` gives a commit-message line. 6 tests on the CardinalGesture example +
  synthetic: enum variant applied + fn kept; impl-fn merged without dropping siblings or duplicating;
  new item inserted; a loose usage snippet REPORTED as unplaced (never silently dropped/misplaced).
  This is the stitch behind cards.rs MergeEdit — the egui popup + diff preview is the remaining wiring.
  Honest limits: pragmatic scanner (common Rust item forms), not a full parser; reports what it can't
  place rather than guessing.

## BATCH 3: tiled decode mat-vec (perf) — 2026-06-12
- CORRECTION to an earlier note: the block forward is NOT host-round-tripping. lib.rs block_forward
  (line ~482) is fully GPU-resident — every op is a `_t` Tensor kernel, no intermediate leaves the
  GPU between layers; `download` only fires for the logits readback and batches into the open frame.
  The ~25× gap vs the M2 bandwidth ceiling (12.8 tok/s on a 2.27 GiB int8 model) is kernel efficiency,
  not transfers.
- DIAGNOSIS: the m=1 decode mat-vec (MATVEC_Q8_0) used ONE WORKGROUP PER OUTPUT COLUMN, so the
  activation vector a[k] is re-read once per output column. For the FFN down-proj (k=8192, n=2048)
  that's ~67 MB of activation reads vs ~17 MB of weight bytes — the kernel was bottlenecked re-reading
  the vector, and the FFN is ~80% of each layer's weight bytes.
- FIX: **MATVEC_Q8_0_TILED** — one workgroup computes `tile` (≤8) output columns, reading each a[kk]
  ONCE and reusing it across the tile. Same weight bandwidth, `tile`× less activation bandwidth. Per-
  column accumulation is unchanged, so it's bit-identical to the untiled kernel.
- A/B TOGGLE: `LOGSCOPE_MATVEC_TILE=N` (1..8), read once at Gpu::new. Default 1 = the proven untiled
  path (unchanged). Set 8 to measure the tiled path. e.g. `LOGSCOPE_MATVEC_TILE=8 cargo r --release
  --bin logscope-egui`. Compare the "tok/s" line against the default run.
- VALIDATED: `--gate-matvec` proves tiled == untiled BIT-EXACT (max|Δ|=0.000e0) across ffn-down,
  ffn-gateup, attn-q, attn-kv (GQA), and vocab>32768, for tiles 2/4/5/8 (5 = non-divisor → exercises
  the col<n guard). End-to-end: `--gate-mmap` is bit-exact (0.000e0) under MATVEC_TILE=8, and the full
  kernel suite passes with the default tile=1. Perf delta is the user's to measure on the M2.
- NOTE: tile reuses the activation but still issues `tile` weight reads per k; the win is the activation
  bandwidth. If decode is weight-bandwidth-bound after this, the next lever is the attention-over-KV
  kernel (the 12.8→10.6 tok/s context-growth cost) and/or fusing the residual adds.

## Feedback pass 2 (dedup bug + model-name log + files drawer) — 2026-06-12
- **Generated-files duplication FIXED** (workspace.rs parse_artifacts; this IS the display path,
  tabs/chat.rs:871). The coalescing was meant only for warm-resume fence splits but also merged two
  COMPLETE programs into one file (two `fn main`). Now it coalesces only ADDITIVE continuations:
  reuses merge.rs scan_items — if the new block REDEFINES an item already present, it's a separate
  file with a unique name (snippet.rs, snippet-2.rs). 13 tests incl. the two-complete-programs
  regression + the existing warm-resume/additive cases. (merge::scan_items made pub.)
- **Model name on every generation log line**: backend.rs Engine carries the loaded file's basename;
  prompt/produced/profile/continued lines now print "[llama-3.2-1b-…gguf]" so a mid-session model
  swap is visible in the log.
- **Generated-files drawer**: tabs/chat.rs wraps the file list + buttons in a CollapsingHeader
  (closed by default, remembers state) so a tall file list no longer blocks the chat; the one-line
  header still shows the count. egui skips the body while collapsed (saves render cost too).
- tile=8 A/B result: REGRESSED on M2 (12.8→10.6) — fewer workgroups starves M2 parallelism. Default
  (LOGSCOPE_MATVEC_TILE unset = tile 1) is the proven path; tiling is the wrong lever for this GPU.

## SYSTEM OPTIMIZATION: autotune decode mat-vec on first load — 2026-06-12
- The tile choice is hardware-specific (M2 wants 1; a bandwidth-bound discrete GPU may want more), so
  instead of a manual env var that regressed on M2, the engine now MEASURES. On FIRST model load
  (after prewarm — NOT at startup, so startup stays snappy), `Gpu::autotune_matvec(k,n)` benchmarks
  the candidate tiles {1,2,4,8} on the model's dominant (FFN-up) shape, frame-batched + median-of-5,
  and installs the fastest. Cached to disk (`<cache>/autotune.tsv`, keyed by adapter+shape) so every
  later load skips the benchmark.
  - Overrides: LOGSCOPE_MATVEC_TILE pins it (skips autotune); LOGSCOPE_NO_AUTOTUNE disables.
  - Logs: "autotuned decode tile = N in Xms (cached…)" first time; "decode tile = N (cached autotune)"
    after; "(LOGSCOPE_MATVEC_TILE override)" when pinned.
  - matvec_tile is now an AtomicU32 (runtime-settable); set_matvec_tile/matvec_tile/tile_forced added.
  - VALIDATED: `--gate-autotune` — returns a tile in {1,2,4,8}, cache get==put + miss==None + overwrite
    (no dup rows), set/read-back, and the chosen tile is BIT-EXACT vs tile 1. (Perf choice isn't
    measurable on lavapipe — it picked 1 there — but the mechanism is proven; the M2 does the real
    selection.) Full gate sweep + kernel suite still green.
- Framework note: autotune is the seam for ongoing per-system tuning. The natural NEXT candidate (and
  the one that might actually move M2 decode, since tile=1 already won there) is a 32-thread split-k
  mat-vec variant (1 warp, matching the M2's warp32) vs the current 64-thread — autotune would pick
  per-GPU. Deferred this turn (it's tolerance-not-bit-exact due to a different reduction order, so it
  needs the argmax-tolerance gate treatment rather than bit-exact).

## Autotune → system-profile matrix (architecture) — 2026-06-12
- Refactored per the "matrix of knowns, not miscellaneous hot-path bits" directive. The decode tuning
  is now a SYSTEM PROFILE: `DecodeConfig` (one coherent kernel config) + `DECODE_PROFILES` (the known
  matrix the autotuner sweeps) + a single `profile` index on Gpu. The decode hot path does ONE lookup
  (`decode_profile()`); adding a tuning dimension = a field on DecodeConfig + rows in the matrix, never
  a new branch/atomic on the path. autotune_matvec sweeps DECODE_PROFILES (not an ad-hoc list).
  Validated: gate-matvec bit-exact, default kernel suite green, env override resolves to a profile,
  app compiles. (gate-autotune mechanism unchanged — still returns a tile in the set + cache round-trip.)

## Coordinator modes (prompt selector UX) — 2026-06-12
- **coordinator.rs** — pure planner for a Barebones / Coordinator / Aggressive-coordinator selector.
  ANSWER to "do we need our own models / a suite?": NO. Each mode is an orchestration PLAN over the
  SAME loaded model run as fabric workers (LocalWorker), assembled by prompt engineering. The "suite"
  is this planner + prompt templates; the parallelism is the existing fabric.
  - Barebones → 1 worker, goal verbatim (today's path).
  - Coordinator → one worker per subtask; each prompt carries the shared context + its own spec + the
    SIBLING units' names/specs (the "partial fill from the other file workers") with "do NOT redefine
    them"; results stitched via merge.rs (Aggregation::StitchFiles).
  - Aggressive → same fan-out + best-of-K per unit (fabric already does best-of-K).
  5 scratch tests: fan-out count, self-excluded sibling list, spec+context threading, best-of-K,
  degrade-to-single when no subtasks. Declared #[allow(dead_code)]; app compiles.
  - Remaining wiring (egui, their test): a Mode dropdown in the chat tab; subtask decomposition (either
    user-listed or one coordinator model call that returns the subtask list); execute plan.workers on
    the Registry; stitch. No new model artifacts needed at any step.

## Right sidebar restructure (UX) — 2026-06-12
- tabs/chat.rs: the per-conversation panels now live in a right SidePanel behind a button rail —
  "☰ Panels" in the top bar toggles it; rail = Files · Context · Prompting (RightPage enum).
  - Context page: the system prompt + backend + generation sliders + ctx-usage bar (MOVED verbatim
    from the old inline context panel; `show_context` repurposed as the sidebar-open toggle).
  - Prompting page: the NEW coordinator::Mode selector (Barebones / Coordinator / Aggressive) with
    per-mode blurbs — selecting records self.coord_mode (execution wiring still to come).
  - Files page: pointer to the existing bottom files drawer for now; full list-relocation is the
    next step once the sidebar layout is confirmed on-device.
  - Compile-checked (cargo check --features wgpu-engine clean); visual layout is the user's to tune
    (sidebar default width 330px, resizable). The bottom files drawer + composer are unchanged.

## Sandbox verify-runner (first slice of the verification loop) — 2026-06-12
- **sandbox.rs** (logscope-egui) — verify a generated workspace by building/testing it in an isolated
  throwaway copy. Rationale (from the Anthropic harness-design + sandboxing notes): for verifiable
  code an LLM evaluator is unreliable and a small local generator can't QA itself, but the toolchain
  is a deterministic oracle — so the "evaluator" is `cargo build`/`test` run over a /tmp MIRROR of the
  workspace under bubblewrap (filesystem + network isolation). Returns {passed, output, sandboxed,
  mirror}; on failure the compiler error is carried verbatim to feed back to the generator.
  - API: VerifySpec::cargo_build/cargo_test/with_command/with_net; verify(&spec)->VerifyResult;
    bwrap_argv(mirror, cmd, net, cargo_home) (pure, testable); mirror_workspace (skips target/ + .git/).
  - bwrap: OS ro-bound, CARGO_HOME ro, the mirror the ONLY writable real path, /tmp tmpfs, HOME=mirror,
    --unshare-net unless net, --die-with-parent. When bwrap is absent → runs in the mirror,
    sandboxed=false (degrades to "throwaway copy, not isolated").
  - 4 scratch tests ✓: mirror skips target/.git; argv unshares net by default + mirror is the writable
    bind + cwd=mirror + toolchain ro + cmd after `--`; net=true drops the unshare; verify reports
    pass/fail from exit status and captures stdout+stderr. Declared #[allow(dead_code)]; app compiles.
  - VALIDATED in-sandbox: mirror + exec + arg-construction + fallback. NOT validatable here (no bwrap):
    the isolation enforcement + `cargo build` under bwrap (toolchain bind paths / registry lock may
    need tuning) — those validate on the Asahi M2.
  - NEXT (deferred, per scope discipline): wire verify() into the branch/score + tree "proven" markers
    + the feed-failure-back-to-generator edge, gated behind the aggressive coordinator toggle. Then
    perf-as-branch-dimension. The conversation-graph spine (durable session log + retrieval-slice
    prompt assembly) is the separate, higher-priority architectural piece.

## Conversation graph spine — visual first, in the sidebar — 2026-06-12
- **convograph.rs** (logscope-egui) — the durable turn-graph + context-window slice. The spine stores
  EVERY turn losslessly (append-only); mark_window(budget) marks only the newest turns that fit the
  prompt budget as in_context (head always kept), the rest stay stored but dimmed. This is the
  substrate behind "the session is not the context window".
  - API: Graph::spine(turns, est) builds a linear chain (parent = prev turn); active_path() = root→head
    lineage; mark_window(budget) -> (in_context_count, tokens_used); window_start() = slice boundary;
    total_tokens / context_tokens. Branches representable via Node.parent (linear today).
  - 5 scratch tests ✓: parent chaining + head; active_path; window keeps newest within budget + dims
    older + boundary; head kept even if alone over budget; token accounting.
- **Sidebar Graph page** (tabs/chat.rs): new "⌗ Graph" tab in the right-sidebar rail. Builds the spine
  from self.msgs each frame, marks the window with the ctx budget, and PAINTS it as a vertical node
  column (egui painter): user=cyan / asst=green dots, parent edges, in-window nodes bright + INK text,
  out-of-window nodes dimmed (linear_multiply 0.30) + DIM text; footer = "N turns stored · M in window
  · ~used/budget tok". Read-only v1 (no click-to-jump yet). cargo check --features wgpu-engine clean.
  - This visualizes the spine; it does NOT yet REPLACE the prompt builder (the model is still fed by
    the existing path). Making retrieval actually drive the fed context (feed only the in_context
    slice, then add relevance-based retrieval beyond recency) is the next step — but now it's visible.

## BUGFIX: UTF-8 char-boundary panic in the runaway-guard tail — 2026-06-12
- Symptom (M2, real run): `panicked at backend.rs:326: assertion failed: self.is_char_boundary(end)`
  on the 2nd turn of a French/Spanish conversation, after prefill, during decode.
- Cause: the runaway-guard rolling tail did `let cut = recent.len() - 1200; recent.drain(..cut);` —
  raw BYTE arithmetic on a String. With multi-byte UTF-8 (accented output: é, í, ñ…), `cut` landed
  mid-character and String::drain asserts a char boundary. Fired on turn 2 because that's when the
  tail first exceeded 1200 bytes. NOT a model bug — detok/streaming/stop handling were all correct.
- Fix: clamp `cut` UP to the next char boundary before draining (both stream paths — the bug was
  DUPLICATED at backend.rs:326 AND :406, the second copy unfixed until now; classic two-copy hazard).
    let mut cut = recent.len() - 1200;
    while cut < recent.len() && !recent.is_char_boundary(cut) { cut += 1; }
    recent.drain(..cut);
- Validated: 3 scratch tests (mid-char cut at byte 401 of an accented string no longer panics + stays
  valid UTF-8; ASCII exact-1200; mixed número/aléatoire). cargo check --features wgpu-engine clean.
- Audit: scanned the crate for sibling String byte-slices; all others (chat.rs think-block splits,
  vertices.rs, merge.rs) use .find()/.position() offsets (char-boundary-safe) or operate on Vec<u8>.

## Graph slice made load-bearing + sandbox oracle wired to the UI — 2026-06-12
### Slice A — convograph now DRIVES the fed context (not just visualizes)
- convograph.rs: added mark_window_ex(budget, sys_tokens, per_msg, max_turns) — prompt-accurate slice
  (reserves system tokens, charges per-turn overhead, caps turns); mark_window delegates to it (pure
  budget). context_ids() = in-context node ids chronologically. +3 tests (turn cap, sys reservation),
  7/7 convograph tests green.
- tabs/chat.rs: chosen_msgs (the ACTUAL prompt history selector) now builds a convo_graph + calls
  mark_window_ex and returns the in_context msgs — byte-for-byte the old recency-within-budget logic,
  but now convograph is the single authority. The Graph sidebar page uses the SAME selection (sys =
  est(sys)+8, llama3 overhead 8, threading cap), so the painted bright/dim split == exactly what's fed.
### Slice B — the oracle grades real generated files
- sandbox.rs: detect_deps (non-std `use`/`extern crate`), scaffold_cargo (1 file w/ fn main → bin,
  else lib + `mod` siblings; extern crates → "*" deps), verify_files(name, files, root) = scaffold +
  `cargo check` (offline for std-only, networked when deps are needed) under bwrap when present.
  +3 tests incl. verify_files_catches_a_real_model_bug_offline which ACTUALLY runs cargo check on an
  E0072 recursive-type snippet (the kind the 1B emitted) and confirms it fails with the diagnostic.
  6/6 sandbox tests green (in-sandbox, unsandboxed-fallback path; bwrap isolation still M2-validated).
- tabs/chat.rs UI: Files sidebar page now lists the latest reply's files + a "⌗ Verify build" button
  that runs verify_files on a BACKGROUND THREAD (VerifyJob: mpsc channel, polled each frame) and shows
  checking…/✓ compiles/✗ build failed + the compiler output (scrollable) + an "unsandboxed" note when
  bwrap is absent. cargo check --features wgpu-engine clean. UX is the user's to validate on the M2.
- NEXT: feed a failed verify's output back to the generator (iterate); then per-branch verify under the
  aggressive coordinator (proven branches pinned). Retrieval can later go beyond recency (relevance via
  context_graph), now that convograph owns the selection.

## Verify outcome classification + UX collapse — 2026-06-12
- M2 verify of dep'd code (rand) failed with "could not resolve host: index.crates.io" — a DEPENDENCY
  FETCH failure, not a build failure, but the UI showed "✗ build failed" (misleading).
- sandbox.rs: added Verdict {Pass, CompileError, DepsUnavailable, ToolError} + classify(passed, output)
  — checks registry/DNS failures FIRST (could-not-resolve-host / failed-to-download / spurious-network /
  failed-to-get / no-matching-package), then rustc diagnostics (error[E…], aborting due to, cannot find,
  mismatched types). +2 tests (the exact M2 network error → DepsUnavailable; E0072 + cannot-find →
  CompileError); end-to-end verify_files test now also asserts the verdict. 8/8 sandbox tests green.
- chat.rs Files page: verify result now shows three states — ✓ compiles (green) / ✗ build failed (red,
  real rustc errors) / ⚠ couldn't fetch dependencies (amber, "the code itself wasn't judged") / ⚠ verify
  error. So a network hiccup no longer masquerades as broken code.
- UX collapse (per the screenshot): removed the standalone top checkpoint/history/tree/export bar; those
  controls (📌 / 🕘N / 🌲N / ⧉) are folded into the composer's bottom-right, left of Send. The toggled
  panels (rollback list, history tree) still render in place. cargo check --features wgpu-engine clean.

## Context-management primitives: relevance beyond recency (exploratory) — 2026-06-12
- Framing: a turn's keep-score = recency + reference-match + (−)supersession. The two avenues the user
  raised ("pull from front on a keyword like undo" and "iteration done, keep the end") are the latter
  two terms, not separate systems.
- convograph.rs primitives (pure, 11/11 tests green):
  - Node now carries `keywords` (content words, stopword-filtered/deduped) + `looks_back` (back-ref cue),
    computed in spine() from FULL turn text (not the truncated preview).
  - keywords(text) / cue(text)->Option<Cue::LookBack> (undo/go back/earlier/first version/remember/…).
  - back_reference_pins(top): if the head looks back, the earlier turns with highest keyword overlap —
    to pull forward into context even if out of the recency window (the "undo keyword" avenue).
  - segments(): contiguous same-topic runs (overlap with predecessor); a run followed by a topic shift
    is a completed iteration. superseded(): every turn except each segment's LAST = collapsible churn,
    so dropping these first keeps outcomes (the "iteration done, keep the end" avenue).
- NOT yet wired into the live selection or Graph view — heuristics (overlap threshold, cue list, what
  counts as "done") need the user's REAL conversations to tune; switching the fed context on untuned
  heuristics would risk pulling the wrong turns. Next (offered): a mark_window_smart = recency ∪ cue-pins
  − superseded-churn, shown in the Graph page (pinned-from-back + collapsed-segment markers) BEFORE it
  drives the prompt, so it can be validated visually on-device, then flipped to load-bearing.

## Offline-first verify (the network line, implemented) + aggressive-mode iteration prompts — 2026-06-12
### Offline-first oracle (reasonable default = hermetic, network opt-in)
- sandbox.rs verify_files(name, files, root, allow_net): ALWAYS runs `cargo check --offline` first
  (hermetic; works for std-only AND deps already in ~/.cargo). Only if that returns Verdict::
  DepsUnavailable AND allow_net retries with networked `cargo check` (resolves + caches for next time).
  Common path = zero network; reach-out is explicit + rare. bwrap_argv now binds CARGO_HOME read-WRITE
  only when net (so a fetch persists), read-only offline.
- +1 test: offline_first_reports_deps_unavailable_without_network (uncached crate, allow_net=false →
  DepsUnavailable, fast). Confirmed in-sandbox that a CACHED dep (rand) resolves offline with NO network
  (45 packages locked) — the offline-first win. 8/8 sandbox tests green.
- chat.rs Files page: "⌗ Verify build" + a "fetch deps" checkbox (default OFF = hermetic). DepsUnavailable
  now reads "⚠ dependencies not cached — tick 'fetch deps' to resolve from crates.io once". Passes
  self.verify_net (copied into the worker thread). The line in practice: offline by default, registry
  reach-out only on explicit opt-in; run-time stays offline (cargo check never executes code).
### Aggressive-mode prompts carry iteration context
- coordinator.rs: sys_worker_iterative() = sys_worker() + "this runs in an ITERATIVE loop: your output
  will be compiled (and may be tested) and you may be asked to revise — favor code that compiles over
  clever guesses, prefer std, choose the simplest correct option when ambiguous." plan() uses it for
  Aggressive workers only (Coordinator keeps the plain prompt — no verify loop there yet). +1 test.
- Also fixed an unnecessary-parens warning in convograph.rs. App clean (remaining warnings are
  pre-existing egui SidePanel/default_width deprecations + one unused `load` fn — not introduced here).

## BUGFIX: sandbox DNS (could-not-resolve-host) + UI compaction — 2026-06-12
- M2: networked verify still failed "Could not resolve host: index.crates.io" even with fetch-deps on.
  Cause: bwrap always makes a fresh MOUNT namespace; we bound /etc (so /etc/resolv.conf is present) but
  on systemd machines that file is a SYMLINK into /run/systemd/resolve/…, which we didn't bind — so the
  resolver config target is missing and glibc can't resolve, even though net=true keeps the HOST network
  namespace (no --unshare-net). Also /lib64 was a hard bind but is ABSENT on aarch64 (Asahi) → could
  abort bwrap.
  Fix (bwrap_argv): hard-bind /usr,/bin,/lib,/etc; TRY-bind /lib64 + the DNS resolver paths
  (/run/systemd/resolve, /etc/resolv.conf, /run/resolvconf, /run/NetworkManager) so DNS works in the
  namespace and missing paths don't abort. +1 test (DNS + lib64 try-binds present, cargo cache rw on net).
  9/9 sandbox tests green. Enforcement still M2-validated (no bwrap here). If it STILL fails after this:
  check whether the result shows "· unsandboxed" (→ bwrap absent, it's the host's own network/DNS, not
  fixable in-app) vs sandboxed (→ deeper namespace/proxy issue).
- UI compaction (per screenshot): the bottom generated-files drawer is now gated on a new `show_files`
  toggle (default OFF — no always-present top row); a compact "⎙ N" selectable in the composer action
  row (left flow, the empty middle space) shows the count and toggles the drawer. The intricate drawer
  body (preview/save/history) is untouched — just wrapped in `if !arts.is_empty() && self.show_files`.
  cargo check --features wgpu-engine clean.

## Graph jump-scroll + prezi walkthrough camera (ported from voiceterm) — 2026-06-12
- **walkcam.rs** (logscope-egui) — ported voiceterm's cinematic follow-camera as PURE logic: damped_step
  (critically-damped spring, ratio 1: converges, never overshoots, preserves velocity), framing_target
  (center an item at a viewport frac, deadband=hold, cut=snap), and a WalkCam {focus(idx)→bool cut,
  tick(dt)→top, settled()}. 5/5 tests (no-overshoot convergence, velocity preserved, centering+cut,
  deadband hold, cut-then-dolly-eases).
- chat.rs Graph page rewrite: nodes are CLICKABLE (Sense::click; pointer y → node id) → set walk_focus +
  scroll_chat_to; a walkthrough bar (◀ N/T ▶ · ⤓ latest) steps the focus; the camera dollies a
  camera-controlled inner ScrollArea (vertical_scroll_offset = walk.top) to the focused node each frame
  (request_repaint while !settled), with an amber ring on the focus. JUMP-SCROLL: the chat-log loop
  takes self.scroll_chat_to and calls ui.scroll_to_cursor(Center) at that message — so clicking a graph
  node (or stepping) scrolls the conversation to that turn. cargo check --features wgpu-engine clean.
  Visual smoothness + nested-scroll feel validate on the M2 (graph wheel-scroll is camera-controlled in
  this version; navigation is by click/step).
- NOT done this turn (scoped, honest):
  * TRUE branching tree — convograph has parent links and the renderer draws parent edges, but the data
    is still a LINEAR spine; real forks need the vault/rollback history wired into convograph (the
    existing "Tree (N)" vault). Then the same walkthrough/jump-scroll/camera work on branches.
  * VOICE pipeline (voiceterm voice.rs ≈ 4.3k lines: audio capture + whisper + wake/speaker) — its own
    integration (audio backend + model + threading), not a drop-in. PLAN: a node-annotation layer
    (text/voice transcript + optional human score per node) that voice.rs feeds via its background
    pop_bg_transcription()/finish_recording() surface — human interjection becomes annotations on tree
    nodes and human SCORES on verify results (the reasoning/result-scoring loop). Build the annotation
    data model first (pure, testable), wire audio after.

## Spatial (3D) message view + mode-driven view swap — 2026-06-12
- **spatial.rs** (logscope-egui) — pure 3D layout + orbit-camera projection. layout(parents): nodes on
  a descending helix (angle ∝ generation), siblings fan apart by angle (branches separate in space);
  linear spine ⇒ a clean single helix. OrbitCam{yaw,pitch,dist,target} with orbit()/zoom()/project()
  (yaw+pitch rotate, perspective divide so nearer nodes draw larger). 5/5 tests: descending helix,
  sibling fan, target→center, nearer-larger + yaw-moves-x, zoom clamp.
- chat.rs spatial_ui(): renders the message graph as an orbitable 3D scene in the CENTRAL area —
  far→near painter's-algorithm sort, depth-scaled radius+opacity, parent edges, near-node labels;
  drag to orbit, scroll to zoom, click nearest node → focus (amber ring) + recenter camera + a
  focused-turn preview strip below. Empty-state friendly.
- Mode-driven swap: new `spatial_view` bool. Central panel early-returns to spatial_ui() when set,
  ELSE the existing linear chat log (untouched — basic chat unchanged). Top-bar "▤ Linear / ◫ Spatial"
  toggle; selecting Aggressive in the Prompting page flips it ON, Barebones flips it OFF (basic→linear,
  aggressive→spatial-relevance, as requested). Bottom composer + sidebar stay in both views. This frees
  the central area (and the linear-list clutter) — the room for the voice layer to land next.
  cargo check --features wgpu-engine clean. Visual feel (helix readability, orbit/zoom, label density)
  validates on the M2 and will want tuning.
- Still pending (unchanged): TRUE branch data (vault history → convograph) so the helix actually forks;
  the voice annotation/score layer (the node-preview strip is the natural place a voice transcript +
  human score will attach). Layout is relevance-agnostic (pure sequence/helix) for now; topic/relevance
  positioning is the refinement that makes it "spatial relevance" in full.

## Spatial framing fix + prune + section color + token heat — 2026-06-12
- FRAMING BUG (the strand sat off-frame, fixed-target camera): spatial.rs now has centroid(),
  bounding_radius(), fit_distance(); spatial_ui auto-frames — target = graph centroid, dist fit to the
  bounding radius — so the graph sits centered and stays centered as it grows (the "protein" feel).
  Drag/zoom/click opt out (spatial_auto/spatial_zoomed flags); a "⊹ Overview" button opts back in.
  2 new spatial tests (auto_frame_centers_and_fits, empty_layout_is_safe). 7/7 spatial, 5/5 walkcam.
- PRUNE (human context curation → more room): convograph Node gains `pruned`; mark_window_ex SKIPS
  pruned nodes (out of the window, frees their budget for other turns); convo_graph() stamps pruned
  from a new Chat.pruned HashSet, so prune flows into BOTH the prompt selector and every view. UI in
  the spatial preview strip: per-turn ✂ Prune / ↩ Restore; controls row: "✂ Prune churn (N)" (bulk-
  prunes convograph.superseded() — the non-final turns of each topic segment) and "↩ Unprune (N)".
  Pruned nodes render hollow + dim in BOTH the spatial view and the 2D Graph page (⌫ marker). New
  convograph test pruning_a_turn_removes_it_from_context_and_frees_budget (compile-verified here).
- COLORIZE SECTIONS: spatial nodes are colored by convograph.segments() (contiguous same-topic runs)
  via a 6-hue palette; user turns keep a thin ink ring so role is still legible.
- TOKEN HEAT (the no-GPU version of "hot paths by tokens"): node radius scales with per-turn token
  count (hot turns up to +70% bigger) on top of the depth cue; near-node labels now lead with "{N}t".
- cargo check --features wgpu-engine + --tests both clean.
- DEFERRED (big, all-visual, need their own pass — NOT built blind):
  * GPU token PARTICLES (poisson-sampled spatially-dithered orbs, periodic numbers) — needs a glow
    paint callback + a particle system; it's a graphics subproject and 100% visual, so it wants the
    M2 in the loop from the first pixel. Groundwork that IS tractable next: a pure poisson-disk
    sampler + per-node particle-count = f(tokens) (testable), then render via glow.
  * FUZZY-FIND pane that drifts through the tree + relevance-driven RE-LAYOUT ("strand bends like a
    protein" toward context) — the fuzzy match + focus is tractable; the live relayout is a force-
    directed/relevance layout subproject. Current auto-centroid-follow is a first taste of the strand
    staying centered + adapting; true relevance positioning is the next layout iteration.

## Real branching + branch UX + progress estimate (tok/s gated) — 2026-06-12
- BRANCHING (fixes "two answers stuffed linearly" — the spine was a flat Vec). convograph gains
  Graph::from_tree(items_with_parent, est, head): two turns may share a parent (a real fork); head =
  the live leaf, active_path follows ITS lineage. Msg gains `parent`; Chat gains `active_leaf` (live
  tip) + `branch_from` (pending fork point). convo_graph() now builds via from_tree using each msg's
  parent (linear fallback = prev id for pre-branch turns) with head=active_leaf, so the live branch is
  whatever lineage the tip sits on — and the prompt (chosen_msgs→active_path) follows that branch only,
  so a side-thread does NOT drag in the other branch. send() forks off branch_from||active_leaf and
  advances the tip; edit/regenerate/delete + all (re)load paths re-seat active_leaf (resync_tree).
  13/13 convograph tests incl from_tree_active_path_follows_the_branch (forks → real siblings; head
  switches the live lineage) — RUN-validated standalone.
- BRANCH UX: spatial focused-node strip has "⑂ Branch here" (sets branch_from); composer shows a
  persistent "⑂ branching from turn N — next message forks a side-thread · ✕ cancel"; the scene marks
  the live tip (cyan ring) and the branch point (purple ring). The immediate fork (siblings of one
  parent) fans apart in the helix; deep sub-branches can still overlap — full tree-lane layout is the
  next refinement (now unblocked).
- PROGRESS ESTIMATE > raw tok/s for normal users: estimate.rs (pure, 4/4 tests) — GenSample{prompt,
  output,secs}; predict(history, cur_prompt, band) matches on prompt-size similarity ("ran with ~67%
  similar inputs → expect ~similar output"), else global fallback; progress() → a clamped fraction.
  Chat banks a GenSample per completed Done; the status line shows "decoding… ~N%" (hover: how many
  prior runs / similar) instead of tok/s. Raw "{n} tok · X tok/s" is now gated behind LOGSCOPE_DEBUG=1
  (both the live and the post-run labels). The loading ring/sphere is the visual next step; the % is
  the functional core.
- NOT built (designed, now unblocked by branch data): AUTO-RECONCILE — when idle/budget allows, re-
  prompt with the human branch context to merge the two branch tips into a corrected synthesis (a
  "merge node"). Needs a 2-parent node (convograph parent is a single Option today) + a coordinator
  reconcile prompt; that's a small model extension + an orchestration pass.
- cargo check --features wgpu-engine + --tests clean.
- M2 run log (smollm2-360m q4_k_m, int8 0.56 GiB): ~13.8–15.5 tok/s, anchor-token REUSE working
  (e.g. reused 733 + prefilled 160/227), GPU-bound, int8 cache hit. The 401t "clone() is a method in"
  node = a big assistant turn rendering fatter (token-heat sizing confirmed on-device).

## Lane-helix layout + fuzz/consensus toolkit — 2026-06-12
- LAYOUT (helix + spacing, keeps the organic spiral, fixes deep-branch overlap): spatial::layout now
  accumulates an angular LANE per node — a fork's children fan over a capped arc and descendants
  INHERIT the divergence, so sub-branches keep their own sector; a linear spine stays a single clean
  helix; many siblings (a fuzz fan-out) pack into the arc without coinciding. Deterministic, no force
  iteration. 9/9 spatial tests (+ deep_subbranches_stay_separated, fuzz_fan_is_all_distinct). Live in
  the spatial view with no UI change (it already calls layout). NOTE: a force-directed "protein"
  relaxation is available as a heavier alternative (voiceterm graph_layout.rs = Fruchterman-Reingold);
  the lane approach gets separation deterministically.
- CONSENSUS TOOLKIT (consensus.rs, pure, 5/5 tests) — the engine for "fuzz the prompt into N branches,
  then merge" (self-consistency / best-of-N as a tree; CAPTCHA-style pooling of noisy reads):
  * fuzz(input,n,seed) → n distinct prompt variants via framing prefixes/suffixes (LCG-seeded,
    deterministic, core preserved) — diversifies a small model's output without changing the ask.
  * cluster(outputs,threshold) → greedy Jaccard token clustering.
  * garbage_score(text) → 0..1 degeneracy (empty/short/looping repeats/low variety) — the small-model
    failure mode (matches the degenerate French-sort transcript).
  * consensus(outputs,thr,cutoff) → drop garbage, cluster the rest, take the LARGEST cluster (what most
    branches converged on), pick its cleanest member; Consensus{chosen,cluster,agreement,kept}. Falls
    back to least-bad if all degenerate.
- NOT wired this turn (designed; the validatable halves are done): the FUZZ ACTION itself —
  hotkey/Enter-variant → fuzz(input,N) → queue N generations as sibling branches off the current node
  → on completion cluster + show consensus + offer merge. Needs a generation QUEUE (serial on the
  local model: N×~15 tok/s is a real wait, but the spatial fan-out makes it watchable) + a cluster→
  message expansion UI (w2q dumb_summary/score_relevance is the reference for grouped-cluster
  summaries). The merge can also feed the auto-reconcile merge-node (2-parent) path.
- crate cargo check --features wgpu-engine clean. Standalone test tally now: walkcam 5, spatial 9,
  convograph 13, estimate 4, consensus 5 — all run-validated.

## Protein-fold layout (literal log-time timeline) + branch-while-busy queue — 2026-06-12
- BUGFIX (branch couldn't fire mid-generation — Send was hidden behind Stop): added a send QUEUE.
  send() now enqueues (text,parent) when self.stream.is_some() instead of blocking; ui() drains one
  queued turn via start_generation() the moment the model is free. Composer shows "↪ Queue" (+ Enter
  works while streaming) and "{n} queued". Non-blocking branching — fire a side-thread without stopping
  the model, it runs when the current reply ends. Queue cleared on Clear/load. start_generation()
  factored out of send() (shared by both paths).
- PROTEIN FOLD LAYOUT (spatial.rs): the timeline is now LITERAL. Vec3 gained add/sub/scale/dot/norm.
  bond_rest(dt_secs) = log-time rest length (1s vs 1h both legible). fold(parents, times_secs, params)
  = a force relaxation seeded by the helix: parent→child BONDS spring to their time-based rest length;
  the backbone has angular RIGIDITY (resists bending toward the grandparent→parent line); every node
  REPELS every other → a long chain folds back on itself and branches separate; roots pinned;
  deterministic. FoldParams{iters,edge_k,bend_k,repel_k,repel_radius,dt,damping}. 12/12 spatial tests
  (+ fold_respects_time_based_bond_lengths [long pause → long bond, bonds near rest], 
  fold_separates_and_is_deterministic, fold_single_node_is_safe).
- WIRED into spatial_ui with a CACHE: fold_pos recomputed only when msg count changes (O(n²·iters));
  times from Msg.born (saturating since the first msg). The auto-frame (centroid+fit) now frames the
  folded shape. cargo check --features wgpu-engine clean.
- NOTES / follow-ups: the fold is a STATIC re-relax on each new turn (a visible snap, not animated) —
  warm-starting from the previous fold_pos + few iters would both smooth it and animate the fold in
  real time (watch it settle). Params are conservative + visually unvalidated → tune on the M2 for the
  right fold feel. O(n²) repulsion is fine at conversation scale; very large trees would want spatial
  hashing. Standalone tests: walkcam 5, spatial 12, convograph 13, estimate 4, consensus 5.

## SCRATCHWORK: bigger-model support (M2 unavailable) — 2026-06-12
- Investigated the engine's model path. Findings: (1) quant dequant is ALREADY broad (F32/F16/Q4_0,1/
  Q5_0,1/Q8_0/Q4_K/Q5_K/Q6_K) — not the blocker; (2) MEMORY is the ceiling — loader materializes f32
  then keeps int8-resident with NO fit check, so oversized models fail late ("70B refused"); (3) ARCH
  is llama-only (hard refuse), with Qwen2 flagged as the roadmap target (just QKV bias).
- NEW pure/tested modules in gpu-gemm:
  * memplan.rs — ModelShape→params, resident_weight_bytes(fmt: F32/Int8/Q4), kv_cache_bytes(ctx),
    peak_stage_bytes, plan(budget,ctx)→Fit{Resident{fmt}|Streaming|TooBig{min}}. 6/6 tests; int8
    footprint calibrated to the SmolLM2 0.56 GiB M2 log. Fit table (39.5 GiB M2): ≤32B fits int8,
    70B→streaming/q4. KEY FINDING: 14B/32B llama-family ALREADY fit int8 and use the same kernels —
    only the refusal blocks them.
  * archcfg.rs — for_arch(str)→ArchConfig (prefix, qkv_bias, norm, act, rope, logit/attn softcap,
    tied_embed, supported, note) + dim_keys (generalizes hardcoded llama.* keys) + qkv_bias_tensors.
    llama supported; qwen2/gemma2/phi3 scaffolded with exact "what's missing" notes. 5/5 tests.
  * Both declared in lib.rs; full crate `cargo check --lib` clean (wgpu build ~1m).
- Design doc: logscope-suite/BIGGER_MODELS.md — 3 axes, the fit table, recommended incremental path
  (1: wire memplan→graded verdict; 2: lift refusal for in-budget llama 14B/32B [no new kernels];
  3: Qwen2 = +bias add, M2-gated bit-exact vs Qwen2-0.5B; 4: q4-resident/streaming for 70B; 5:
  gemma2/phi3 later), and an explicit M2-gated list + a suggested --gate-memplan.
- Standalone test tally now: walkcam 5, spatial 12, convograph 13, estimate 4, consensus 5,
  memplan 6, archcfg 5.

## Modal (vim-like) control: palette/search/jump/camera + colon commands — 2026-06-12
- NOTE: the log-time protein-fold layout and the branch-while-streaming QUEUE were both shipped the
  prior turn; not redone. This turn adds the modal layer on top, scoped to the SPATIAL view.
- vimmode.rs (pure, 6/6 tests): Mode{Normal,Insert,Command,Search}; parse_command (aliases :b/:branch,
  :a/:add/:attach <path>, :goto N / bare :N, :prune/:unprune, :overview/:fit, :fuzz N, :linear/:spatial);
  fuzzy_score (fzf-lite subsequence, contiguity-dominant) + fuzzy_rank; Action enum + normal_action
  (maps a key + z-prefix + space-leader → action). Tests cover command aliases, fuzzy subsequence +
  ranking, nav keys, camera keys under z-prefix, leader + search keys.
- chat.rs wiring (compile-checked; FEEL UNVALIDATED — needs M2):
  * handle_modal(): in Normal mode reads Event::Text chars → normal_action → exec_action; Esc → Normal
    (+ surrender composer focus); Enter → Insert (focus composer). z/space are armed prefixes.
  * Keys: j/k move focus along the live branch (active_path), h/l switch sibling branch (and make it
    live via deepest descendant), g/G first/last, i/I zoom, z+h/l yaw + z+k/j pitch, / search,
    space-/ tree search, space-\ fuzzy tree, : command line, a (or Enter) insert, b branch, o overview.
  * Colon commands via the status-bar input → run_command. :fuzz N WIRED to consensus::fuzz + the
    branch queue → fans N framing-perturbed variants as branches off the focused node (the fuzz + queue
    + branching pieces working together; the consensus MERGE of results is the remaining follow-up).
  * Search: live fuzzy jump (fuzzy_rank over message previews → set_focus to best).
  * Thin 18px status bar (Panel::bottom "vim_status", created before the composer so it sits at the
    very bottom): mode chip + live :/ buffer + key cheatsheet + turn indicator. Only shown in spatial.
- CAVEATS: scoped to spatial view (linear chat unchanged). The egui focus handoff (Normal↔Insert, Text
  events only reaching us when nothing's focused, Esc release) is the fiddly part and the most likely to
  need iteration on the M2. Full in-message vim text-motions are a deeper follow-up.
- Standalone tests: walkcam 5, spatial 12, convograph 13, estimate 4, consensus 5, memplan 6, archcfg 5,
  vimmode 6.

## memplan WIRED into the loader + --gate-memplan + tree export + double-Esc + touch mapping — 2026-06-12
- memplan loader guard (lib.rs load_gguf_opt, after kv computed): cheap metadata-only vocab estimate
  (tensor dims, no dequant) → if store::resident_budget_bytes() is known, builds memplan::ModelShape and
  computes need = resident_weight_bytes(int8 if quantizing else f32) + kv_cache_bytes(ctx 4096). If it
  exceeds budget, RETURNS Err WITH the GiB number + a plan()-based suggestion (q4 would fit but isn't
  implemented / streaming / use llama.cpp). Reuses the EXISTING store.rs readers (available_memory_bytes,
  resident_budget_bytes, LOGSCOPE_MAX_RESIDENT_GB) — no duplication. Refuses gracefully instead of OOMing;
  in-budget llama (14B/32B int8) was never refused on memory and still loads.
- --gate-memplan (main.rs, pure, no GPU): RAN IN-SANDBOX, all 9 checks ✓. Fit table prints
  8B→Resident Int8 (8.9 GiB weights), 70B→Streaming (int8 68.6 GiB > 39.5). Asserts param counts, that
  the int8 estimate matches the M2 SmolLM2 ~0.56 GiB anchor, ≤32B fits int8 / 70B doesn't, q4<int8, kv
  scales linearly. Resolves the user's `unknown gate '--gate-memplan'`. Added to the help line.
- DOUBLE-ESC (chat.rs handle_modal): consumes Esc (so a focused composer doesn't also eat it); double-Esc
  within 450ms TOGGLES motion(Normal) ↔ Insert; single Esc cancels Command/Search or clears a pending
  prefix. Clicking the composer (it gains focus) flips to Insert. THIS IS THE FIX for "keybinds weren't
  working" — focus never left the composer so Normal-mode Text events never reached the handler. Bottom
  bar shows "typing — Esc Esc → motion mode" in Insert.
- TREE EXPORT (treeexport.rs pure, 4/4 tests): ExportNode{id,parent,role,text}; ascii() (├─/└─ connectors),
  markdown() (nested, branches labelled), git_script() (a bash script that reconstructs the tree as real
  git history — one commit per turn, one branch per fork, each commit tagged n<id> so children check out
  their parent; turn text via a quoted heredoc). Wired as a :export <ascii|md|git> command (also :e) →
  builds the WHOLE tree from convo_graph + writes <autosave>/exports/tree-<unix>.{txt,md,sh}; the path
  shows in the status bar. (The existing ⧉ button still does the linear-Markdown export, untouched.)
- TOUCH MAPPING (vimmode.rs pure, in the 7/7): Dir4 + joystick_dir(dx,dy,deadzone) (dominant axis past
  deadzone), nav_dir_action (L/R = switch branch, U/D = crawl timeline), cam_stick(dx,dy,sens) →
  (Δyaw,Δpitch). LOGIC ONLY — the egui touch widgets (two on-screen joysticks + multitouch 3D drag +
  is-touch detection) are NOT built yet; this is the testable core they'll sit on.
- Standalone tests now: walkcam 5, spatial 12, convograph 13, estimate 4, consensus 5, memplan 6,
  archcfg 5, vimmode 7, treeexport 4.

## docs/ folder + frontend/backend-split TODO — 2026-06-12
- --gate-memplan confirmed GREEN on the M2 (user paste): all 9 ✓, fit table 8B→Resident Int8 (8.9 GiB),
  70B→Streaming. Memory planner validated on real hardware.
- TODO.md: added "Frontend / backend separation + a headless TUI" — lift the conversation core
  (convograph/prompt-assembly/estimate/consensus/fuzz/treeexport/memplan/modal logic) out of the egui
  app into a frontend-free crate, define one backend boundary (in-proc Inference trait OR logscope-serve
  over the wire), build logscope-tui (ratatui+crossterm) reusing treeexport::ascii as the tree renderer
  + vimmode mapping 1:1 to the terminal. Most of the brain is already GUI-free thanks to the pure-module
  discipline; mostly moves files + one trait.
- docs/ (new, markdown, ~2.6k words): explains the ideas behind the test suite — README (the
  small-model-plus-structure thesis + the "fast path == slow path" equivalence discipline), then
  01 numerical ground truth (CPU oracle, bit-exactness), 02 prefix reuse + branching (KV trie = the
  conversation graph is real, forking is ~O(new tokens)), 03 speculative + batched decoding (spec ==
  greedy to 0.0; KV-budget admission control; int8-KV ~4× budget), 04 consensus/fuzz/verification (the
  thesis core: cheap samples + deterministic judges — rubric scorer alt-matching, compiler oracle,
  human prune), 05 quantization + memplan (lossy-but-argmax-faithful int8, the fit table, refuse-with-a-
  number). Each chapter is grounded in actual validate-suite output lines.

## mount.rs — isolated scratch workspaces + git-bundle handoff (system integration) — 2026-06-12
- THE missing-link integration: work on a real project safely. detect(path) → CargoProject/Workspace/
  SingleFile/PlainDir + source_is_git. isolate(mount, scratch_root) → mirror the working tree (skip
  target/.git) into tmp/logscope/{uuid}, fresh `git init`, baseline = exact mounted state (incl.
  uncommitted edits) tagged logscope-base on branch logscope-work. snapshot(ws,msg) → commit the agent's
  changes (no-op if nothing changed). bundle(ws,out) → a cloneable git BUNDLE (HEAD+work branch) + a
  PATCH (git diff base..work) + the changed-file list. apply_hint → exact commands (clone the bundle to
  review / git apply the patch onto your tree / git fetch the branch in).
- WHY bundles: the bundle/patch are the ENTIRE interface between the sandbox and the user's real tree —
  nothing is written back automatically. Bundle = self-contained, cloneable; patch = applies straight
  onto the working tree. Matches "applied or cloned back for staging changes outside the workspace".
- VALIDATED END-TO-END in-sandbox with real git 2.43 + cargo (2 standalone tests): the roundtrip mounts
  a cargo project, isolates, the agent edits src/main.rs + adds NOTES.md, snapshots, bundles — then (1)
  `git clone <bundle>` yields a standalone repo carrying the edit + new file, and (2) `git apply <patch>`
  reproduces both onto a FRESH copy of the source. Asserts the original tree is never modified. Plus a
  detect() classification test. Fixed: bundle needs HEAD to travel or a bare clone checks out nothing.
- Lives in logscope-egui next to sandbox.rs (the compiler oracle) for now; both are backend/core and are
  flagged in TODO to move into the shared crate in the frontend/backend split.
- Standalone tests: walkcam 5, spatial 12, convograph 13, estimate 4, consensus 5, memplan 6, archcfg 5,
  vimmode 7, treeexport 4, mount 2.

## Qwen2 support — BIGGER_MODELS step 3 (arch unlock) — 2026-06-12
- THE engine work: Qwen2 = llama + a Q/K/V bias add + reading qwen2.* metadata. Implemented:
  * Loader reads dims by ARCH PREFIX (general.architecture → "{pfx}.embedding_length" etc.), so qwen2.*
    keys (incl. qwen2.rope.freq_base = its RoPE θ) are read generically. Refusal lifted for qwen2 (still
    refuses gemma2/phi3/unknown).
  * Q/K/V bias loaded per layer (blk.N.attn_{q,k,v}.bias) when present.
  * `Lin` enum→struct: `{ kind: F32|Q8, bias: Option<Tensor> }`. apply() folds the bias in AFTER the
    matmul, so ALL ~9 forward paths (prefill/decode/batched/ext) get it for free with zero call-site
    changes. New broadcast kernel `ADD_BIAS` (out[i] = a[i] + bias[i % cols]).
  * Cache untouched: biased models SKIP the int8 cache write (save_int8_cache early-returns if any Lin
    has a bias) → cache format + gate-store/mmap identical for llama; qwen2 just reloads from GGUF.
- VALIDATED IN-SANDBOX (lavapipe = correct math): `--gate-qwen2` 5/5 — add_bias_t bit-exact vs CPU;
  qwen2 GGUF loads + qwen2.* dims read (zero-bias == llama path); nonzero bias changes the logits; int8
  qwen2 argmax == f32; gemma2/other still refused. Added write_gguf_qwen2() (qwen2 GGUF + bias tensors).
  Full kernel suite still "ALL KERNELS VALIDATED" + gate-store green → the Lin refactor didn't regress.
- archcfg: qwen2 supported=true now (test updated). M2-GATED remainder: bit-exact vs a real
  Qwen2-0.5B/1.5B (converter quirks). BIGGER_MODELS.md path updated: steps 1-3 done; 4 (q4-resident /
  layer streaming for 70B) + 5 (gemma2/phi3) remain — ADD_BIAS + Lin-bias plumbing are reusable for both.
- Standalone tests: walkcam 5, spatial 12, convograph 13, estimate 4, consensus 5, memplan 6, archcfg 5,
  vimmode 7, treeexport 4, mount 2. Engine gates: +--gate-qwen2 (5 checks).

## q4-resident matmul — BIGGER_MODELS step 4 (memory lever) — 2026-06-12
- THE memory lever memplan flagged ("would fit at q4 but q4-resident isn't implemented"). Now real:
  * Q4Weight (per-32-block f32 scale = amax/7, symmetric nibbles value−8 ∈ [−7,7], 8 nibbles/u32) —
    8× smaller than f32, 2× smaller than int8. My own resident format (not GGUF q4_0's f16 scale) so
    CPU and GPU dequant identically → bit-exact.
  * quantize_q4_resident + linear_q4_0_t (dispatches MATVEC_Q4 for decode m=1, LINEAR_Q4 for prefill
    m>1) — kernels mirror the q8_0-resident ones, reading nibbles via extractBits(word,(kk%8)*4,4) and
    dequant (nibble−8)*scale.
  * Wired through Lin::Q4 (LinKind gained Q4) → apply() routes to linear_q4_0_t; int8-cache dump/read
    gained tag 2 (backward-compatible: tags 0/1 unchanged). Opt-in via LOGSCOPE_Q4_RESIDENT=1 in
    make_lin. Cache bypassed in q4 mode (save + load guard) so a q4 load can't pick up a q8 cache.
- VALIDATED IN-SANDBOX: --gate-q4resident 5/5 — q4 mat-vec (m=1) + linear (m=5) bit-exact vs CPU
  dequant+matmul; q4 lossy-but-bounded; 70B int8 68.6 GiB → q4 40.3 GiB (planner's q4 path now real); a
  FULL model loads q4-resident through the loader + tracks f32 (cos=0.9119). Full kernel suite still
  "ALL KERNELS VALIDATED", int8 cache reload bit-exact, gate-store/mmap/qwen2 green → no regression.
- M2-GATED remainder: q4 mat-vec THROUGHPUT on AGX (memory-bound decode kernel; correctness proven,
  speed hardware-shaped), and whether 40 GiB fits 39.5 with KV+compute headroom (borderline → smaller
  ctx). Layer streaming (for models too big even at q4) still TODO — IO/perf-shaped, genuinely M2-gated.
- BIGGER_MODELS.md path: steps 1-3 + step-4 q4-resident done (correctness); layer streaming + gemma2/phi3
  remain. Engine gates now: +--gate-q4resident (5 checks). Standalone test tally unchanged.

## Direct q8_0 → int8-resident load (q8_0 wiring for Qwen2.5-Coder-14B) — 2026-06-12
- WHY: q8_0 (GGUF) and our int8-resident QWeight are the SAME scheme (per-32-block scale + i8). The old
  path dequantized every q8_0 tensor to f32 then re-quantized to int8 — wasteful + slightly lossy
  (per-block re-normalization). Direct path loads q8_0 straight into QWeight: exact + faster (no
  re-quant of ~14B values) + no f32 materialization of the big weight tensors.
- q8_0_to_resident(raw,n,k): reads 34-byte q8_0 blocks (gguf::f16_to_f32 scale + 32×i8 = round(w/d))
  straight into QWeight (scale f16→f32, i8 packed 4/u32). Block layout (r*kb+b) matches MATVEC_Q8_0.
- Loader: a `lin(name,n,k,quantize)` selector replaces make_lin for the non-fused projections
  (wq/wk/wv/wo/wdown/output). When quantize && !q4_mode && tensor is q8_0 (ggml_type 8) && k%32==0 →
  q8_0_to_resident on the mmap'd bytes; else dequant-then-quantize as before. Fused wgate_up stays on
  the f32-concat path (two q8_0 tensors merged). f32 models are unaffected (type != 8 → old path).
- VALIDATED IN-SANDBOX: --gate-q8direct 4/4 — direct QWeight mat-vec (m=1) + linear (m=5) bit-exact vs a
  CPU matmul over the q8_0-dequantized weights (ZERO error beyond q8_0); a full q8_0 GGUF loads through
  the direct path and tracks f32 (cos=0.99987, near-lossless) with matching argmax. Added enc_q8_0 +
  write_gguf_q8_0 test helpers. Full kernel suite still "ALL KERNELS VALIDATED"; int8 cache + qwen2 + q4
  + memplan gates green → no regression to the shared load path.
- MEMORY (Qwen2.5-Coder-14B, 152K vocab, 48L, GQA 40/8): int8 weights ~15 GiB + f32 embedding ~3 GiB ≈
  18 GiB resident; KV at 4K ctx ~1.6 GiB (f32) → ~20 GiB total, very comfortable on the 64 GB laptop
  (~39.5 GiB avail). Full 32K ctx: f32 KV ~13 GiB (→31 GiB total, fits) or int8-KV ~3.2 GiB (→21 GiB).
  The 192 GB desktop is for 32B q8_0 (~32 GiB weights) or 70B — not needed for the 14B.
- Engine gates now: +--gate-q8direct (4 checks). Test tally unchanged.

## Perf levers for the 14B (prefill tiled GEMM + decode tile override) — 2026-06-12
- MILESTONE: Qwen2.5-Coder-14B-Instruct-q8_0 runs on the M2 (real hw) — qwen2 + q8_0-direct wiring work
  end-to-end, coherent tokens. But slow: prefill 835 tok in 241s; decode 1.02 tok/s, GPU-bound (sync
  6393ms ≫ setup 61ms), 26 submits/tok; autotune chose decode tile=1.
- DIAGNOSIS: (a) prefill uses the NAIVE 8×8 q8 GEMM (LINEAR_Q8_0) — re-reads A/W from global per output →
  the 241s. (b) decode tile=1 re-reads the full activation per output column; for the FFN (k=13824) that
  activation traffic dwarfs the weight reads → bandwidth-bound at ~1 tok/s.
- TWO LEVERS (both correctness-validated in-sandbox via --gate-perf; SPEED is M2-measured; A/B-toggleable):
  * LINEAR_Q8_0_TILED (new): shared-memory-tiled 16×16 prefill GEMM. Each thread accumulates k in order;
    A/W tiles staged in workgroup memory + reused 16×. Matches the naive GEMM within float tol (max|Δ|
    1.5e-5 — differs only by multiply association a·(s·q) vs (a·s)·q, fundamental to dequant-on-load).
    Opt in: LOGSCOPE_PREFILL_TILED=1. Also callable as linear_q8_0_tiled_t.
  * LOGSCOPE_DECODE_TILE=N (1..8): overrides the autotuned decode tile. The tiled mat-vec
    (MATVEC_Q8_0_TILED) reuses each activation across N columns. BIT-EXACT to untiled for N∈{1,2,4,8}
    (gate max|Δ|=0). For a 14B, N=8 cuts FFN activation-read bandwidth ~8×.
- BUG FOUND + FIXED: MATVEC_Q8_0_TILED's acc/red are sized for tile≤8; the override previously clamped to
  32, so N=16 read acc[15] OOB → garbage (2e24). Autotune only ever picks {1,2,4,8} so it was latent;
  --gate-perf is the first correctness test of the tiled mat-vec at multiple tiles. Override now clamps to 8.
- EXPECT: prefill-tiled = the big win (the 241s); decode-tile a moderate win. Neither closes the gap to
  llama.cpp's Metal (years of tuning) — these are the obvious levers. Engine gates now: +--gate-perf.

## Modal keybinds fix — root-caused the "keys don't work" (2026-06-12)
- REPORTED (twice): in the spatial view, vim motion keys didn't register. Read the actual egui flow
  rather than guess. handle_modal runs at the top of ui() (before the composer TextEdit is built later
  the same frame), which is the RIGHT order — but two real bugs remained:
  1. Only DOUBLE-Esc entered Normal (single Esc did nothing). Non-obvious + finicky (450ms window, must
     already be in Insert). After clicking the box to type, the user was stuck in Insert → motion keys
     just typed into the box. This was the main culprit.
  2. handle_modal read Event::Text NON-consuming (ui.input), so even in Normal mode the composer — if it
     held focus — saw the same Text events and typed the motion keys.
- FIX (chat.rs handle_modal):
  * Single Esc → Normal (vim-standard); consume the Esc Key so a focused TextEdit doesn't also react.
    Removed the double-Esc machinery + the last_esc field.
  * In Normal mode, DRAIN Event::Text from the queue (i.events.retain) before the composer is built.
    Because handle_modal runs first in the frame, draining is what actually stops the box from typing
    the motion keys — independent of focus. This is the robustness fix: keys can't leak even if the box
    somehow holds focus. Also surrender any stray composer focus, and consume Enter (→ enter_insert).
  * Insert-mode status hint updated "Esc Esc" → "Esc".
- VISIBLE FEEDBACK confirmed in code: move_focus initializes from None on the first j/k (highlights the
  first/last node + recenters camera), the 18px status bar shows the mode chip (▌NORMAL/▌INSERT, colored)
  + "turn N/M" + the key cheatsheet — all rendered in the spatial view. So a working keypress is visible.
- Compiles with --features wgpu-engine. BEHAVIOR is still M2/interactive-gated (can't unit-test egui input):
  to verify — enter the spatial (aggressive) view, press Esc once → chip shows NORMAL, then j/k should move
  the highlighted node and update "turn N/M"; press a (or Enter) → INSERT, type, Esc → back to NORMAL.

## Shape-keyed, persistable autotune cache (foundation for load-time tuning + multi-model) — 2026-06-12
- WHY: the old autotune resolves ONE global decode tile (an atomic profile index) from whatever shape got
  benchmarked — that's how it landed on tile=1, which starves the 14B's wide FFN-down (k=13824) of
  activation reuse. The best tile scales with k, so it must be PER-SHAPE.
- BUILT (lib.rs, hot path = one map read; writes only at load):
  * Gpu.shape_tiles: RwLock<HashMap<(n,k)->tile>>. decode_tile_for(n,k): env override → per-shape cache →
    global profile. Wired into linear_q8_0_t's m==1 path.
  * autotune_shapes(shapes): probes each DISTINCT (n,k) once (reuses autotune_matvec's warm+median loop),
    caches the best tile. model_decode_shapes(model) reads the distinct shapes off a loaded model (attn
    QKVO + fused gate/up + down + output) so callers don't hand-enumerate. set_shape_tile for manual pins.
  * save_autotune/load_autotune: tiny on-disk format (count + (n,k,tile) triples), CALLER keys the path by
    adapter (tuning is per-GPU, weight-independent — mirrors the int8 cache pattern). Reused across launches
    AND across models that share a shape — two models with the same FFN shape share its tuned tile (the
    multi-model payoff: tune once per shape, not per model).
- VALIDATED IN-SANDBOX (--gate-autotune-cache 6/6): enumerate→tune(5 distinct shapes)→lookup(+global
  fallback)→persist→reload into a fresh GPU→a cached tile routes through decode BIT-EXACT to default. Tile
  QUALITY is M2-measured (lavapipe timing is noise); the MECHANISM is what's proven here. Full kernel suite
  + perf + q8direct gates still green. Engine gates now: +--gate-autotune-cache.
- DESIGN NEXT (discussed, not built): (a) GEMM-config autotuning needs the kernels PARAMETERIZED — wgpu
  pipeline-overridable constants (TILE_M/N/K as `override`s) so configs sweep without recompiling WGSL;
  key the cache by (shape, m-bucket); roofline-prune the sweep; time-box the probe. The shape cache
  generalizes to shape->{decode_tile, gemm_cfg}. (b) Multi-model: a registry (load/unload + aggregate
  resident budget extending store.rs) + a single-queue scheduler that encodes per-request on worker threads
  and submits CHUNKED work round-robin/priority so a big prefill can't head-of-line-block an interactive
  decode (the 26-submits/token granularity helps); shared pipeline cache means heterogeneous models reuse
  compiled kernels + tuned configs; KV budget with LRU-session eviction (paged-KV is the heavier version).

## GEMM specialization via wgpu override constants (prefill tile autotuning) — 2026-06-12
- WHY: the tiled prefill GEMM (the 241s lever) was hardcoded 16×16. To AUTOTUNE the workgroup tile we need
  many pipeline variants from ONE kernel — string-templating WGSL defeats the ptr-keyed pipeline cache, so
  the right mechanism is wgpu PIPELINE-OVERRIDABLE CONSTANTS.
- BUILT (lib.rs):
  * LINEAR_Q8_0_TILED now declares `override TILE: u32 = 16u` and uses it in @workgroup_size(TILE,TILE,1) +
    all loops/indexing. Workgroup arrays are sized for the MAX tile (32→1024 f32 each, 8 KB); a smaller
    tile uses the leading corner. Accumulation stays k-in-order, so EVERY tile is bit-exact to every other
    and all match the naive GEMM within float tol (the a·(s·q) vs (a·s)·q association).
  * gemm_pipeline(tile): builds/caches a pipeline specialized via PipelineCompilationOptions.constants
    (wgpu 29: `constants: &[(&str,f64)]`, keyed by the override NAME "TILE"; zero_init workgroup mem off —
    we write every used entry before reading). Keyed (shader, 0x4000|tile) so it never collides with the
    binding-count pipeline keys. dispatch() was split into dispatch()+dispatch_with(pipe,…) so specialized
    pipelines share the exact bind-group + frame-batch path.
  * linear_q8_0_gemm(a,w,m,tile) dispatches at an explicit tile; linear_q8_0_tiled_t now picks the tile via
    gemm_tile_for. gemm_tile_candidates() filters {8,16,32} by device limits (so TILE=32 is dropped on a
    256-invocation device instead of failing). autotune_gemm(shapes,m_probe) sweeps the candidates per
    DISTINCT shape at a representative prefill width and caches the fastest. gemm_tile_for = env override
    (LOGSCOPE_GEMM_TILE) → per-shape cache → 16. set_gemm_tile for manual pins. Separate gemm_tiles map
    (decode mat-vec and prefill GEMM have different optima).
  * Persistence (save/load_autotune) extended to BOTH caches — versioned ("LAT1"), old decode-only files
    still load. Tune once per GPU, reuse across launches AND models that share shapes.
- VALIDATED IN-SANDBOX (--gate-gemm-spec, all ✓): override constants compile under naga + yield one
  distinct pipeline per tile; device candidates [8,16,32]; every tile matches naive within tol (max|Δ|
  1.5e-5) AND is bit-exact to the others across shapes incl. non-multiples of the tiles; autotune_gemm
  tunes each distinct shape; gemm_tile_for/set_gemm_tile route correctly; GEMM tiles persist + reload.
  Full kernel suite + perf + autotune-cache + q8direct + qwen2 gates still green (the dispatch refactor
  touches every kernel — no regression). Engine gates now: +--gate-gemm-spec.
- M2 NEXT: the tile QUALITY is still M2-measured (lavapipe timing is noise). To activate at load: call
  autotune_gemm(model_decode_shapes(&model), m_probe) + autotune_shapes(...) after load, persist keyed by
  adapter. Or A/B by hand: LOGSCOPE_GEMM_TILE=8|16|32 with LOGSCOPE_PREFILL_TILED=1. Likely-next perf work
  beyond tile sweep: rectangular tiles / register blocking (each thread computes a small output micro-tile),
  vectorized (vec4) loads, and m-bucketed configs (prefill width varies) — all slot into the same cache as
  shape→{decode_tile, gemm_cfg}.

## Autotune wired into model load + logging; docs ch.6 — 2026-06-12
- DOCS: added docs/06-autotuning-and-kernel-specialization.md (+ README index) in the established voice —
  opens with --gate-gemm-spec/--gate-autotune-cache output, then: why one global tile fails, per-shape
  keying, the bit-exact-across-tiles discipline (a config knob must not change outputs) incl. the 2e24 OOB
  the multi-tile gate caught, override-constant specialization, the per-GPU/per-shape persistent cache, and
  the sandbox-correctness vs M2-perf separation.
- AUTOTUNE NOW RUNS AT LOAD (logscope-egui/backend.rs, after the existing global-tile block):
  * load_autotune(cache) first (cache = store::cache_root()/autotune-shapes-{adapter}.bin), then
    autotune_shapes (decode tiles) + autotune_gemm (prefill GEMM tiles) probe only UNCACHED shapes, then
    save_autotune. autotune_shapes/autotune_gemm now skip shapes already in their map, so a loaded cache
    does ZERO re-probing (gated tests: "a loaded shape is NOT re-probed").
  * GEMM probe only runs when LOGSCOPE_PREFILL_TILED is set (its tile is unused otherwise). m_probe =
    model.d.clamp(64,512) (representative prefill width; m-bucketing is the future refinement).
  * LOGGING (visible on the M2): "loaded N decode + M GEMM tiles from cache", "probed N decode + M GEMM
    shapes in Xms (cached to …)", then a per-shape table "shape n=… k=… → decode tile T [· GEMM tile T]",
    then the override hint line (LOGSCOPE_DECODE_TILE / LOGSCOPE_GEMM_TILE / NO_AUTOTUNE / PREFILL_TILED).
- VALIDATED: both crates compile (engine lib + app --features wgpu-engine); --gate-autotune-cache and
  --gate-gemm-spec green incl. the new skip-cached checks; full kernel suite still "ALL KERNELS VALIDATED".
  Tile QUALITY remains M2-measured; the load-time wiring + logging are what let the M2 run report its picks.

## Generalized conversation model — participant DAG, perspective assembly (multi-user/agent/fan-out) — 2026-06-12
- ASK: drop the alternating user/assistant assumption. Any participant (a specific human OR a specific
  agent) can branch off any node; consecutive same-role messages can chain as context; branches can
  coexist (fan-out). One primitive for aggressive fan-out + multi-user rooms + multi-agent interaction.
- GAP ANALYSIS (what the code has vs needs):
  * HAVE: branch-anywhere (Msg.parent: Option<usize> + branch_from), fan-out (sibling children +
    per-message `variants` + :fuzz), chaining (linear parent paths). The tree already supports
    "chained" and "separate-at-once".
  * MISSING 1 — participant identity: Msg.role is binary User/Assistant, so two agents both collapse to
    "assistant" and two humans are indistinguishable. Need an AUTHOR id per message.
  * MISSING 2 — non-alternating assembly: build_prompt_{chatml,llama3,plain} hardcode role==User?
    "user":"assistant"; consecutive same-role would emit adjacent same-role blocks.
  * MISSING 3 — multi-PARENT context: parent is a single Option, so two separate branches can't be
    merged into one node's context. Same primitive as the consensus 2-parent merge; the bigger lift.
- BUILT (logscope-egui/multiparty.rs, pure, std-only, 8 standalone tests ✓) — the foundational fix for
  gaps 1+2: assemble(lineage:&[(author_id,text)], me, name_fn, sys, Template{ChatML|Llama3|Plain}) ->
  prompt, from participant `me`'s PERSPECTIVE: me→assistant, others→user; name-tag non-me blocks ONLY
  when >1 distinct other participant (else it's the ordinary 2-party transcript); MERGE consecutive
  messages sharing (role,tag) so a run of same-author turns reads as one chained block. Plus
  Participant{id,name,is_agent} and other_participants(). Tests cover: 2-party==plain alternation,
  consecutive-merge, multi-agent tagging, perspective flip with `me`, Llama3+Plain, multi-user
  tag+merge, empty lineage. Registered in main.rs (dead_code; not yet wired).
- WIRING PLAN (to fully activate; deferred — user wants to return to perf/large-model testing first):
  generalize Msg.role→author:u32 (0=local human, 1=primary agent, more for extra participants; keep a
  role-class for UI colour), hold a small participant registry, replace the binary mapping in
  build_prompt_* with multiparty::assemble(active_path lineage, me=generating-agent-id, …), let the UI
  add participants + choose who speaks next. Gap 3 (multi-parent) = change parent to a set + topo-merge
  lineage; do alongside consensus-merge. multiparty::assemble is unaffected (it already takes a flat
  lineage, however it was resolved). No engine/kernel changes; full kernel suite + app build still green.

## Autotune was the 3-minute load hang — capped-width probe + time-box + per-shape timings — 2026-06-12
- M2 REPORT (photo): load "locked up" — the per-shape autotune line read "probed 5 decode + 5 GEMM
  shapes in 178351ms" (~3 min). Cause: autotune_gemm built + ran the kernel at the model's FULL output
  width (n=152064) and FFN widths (n=27648) many times, on the already-slow engine. Decode tile came
  out 1 for ALL shapes; GEMM tile 32 for most, 16 for the n=152064 output (so GEMM DID differentiate;
  decode-tile may genuinely not help at m=1 on the M2 — a thing to explore, not a bug).
- FIX (the tile winner is set by k + kernel overhead given GPU saturation, NOT by n, so probe narrow):
  * PROBE_N_CAP=4096 — autotune_matvec caps n internally; autotune_gemm probes at n.min(4096) and caps
    m to ≤256. The big output-GEMM probe drops ~37×. Estimated total probe now ~8–10s vs 178s.
  * HARD TIME-BOX: backend passes a 20s deadline; autotune_shapes/autotune_gemm check it before each
    shape and stop, leaving the rest at defaults (logged as a NOTE). So a load can never stall like this
    again regardless of model size or probe-cost estimates.
  * Both autotunes now RETURN Vec<(n,k,tile,probe_ms)>; backend logs per-shape "probed decode/GEMM
    n=… k=… → tile T in Xms" + a summary "(… ms work, … s wall)". The per-shape timing the report asked
    for is now visible.
- CACHE NOTE: the 178s run already CACHED correct tile values (capping doesn't change the winner, only
  probe speed), so the user's next launch LOADS instantly — no re-pay. The fix protects future fresh
  tunes (new GPU/model/shape). To see the new timing logs, delete the cache (autotune-shapes-*.bin under
  the cache dir) and reload.
- VALIDATED: engine builds; --gate-autotune-cache + --gate-gemm-spec green with the new signatures (5
  shapes each, skip-cached still holds); app builds --features wgpu-engine. No kernel/correctness change
  (capping + time-box + return type only). Tile QUALITY still M2-measured.

## Submit-batching knob: matrix + adaptive tuner + logging (decode-overhead lever) — 2026-06-12
- WHY: M2 decode is ~24-26 submits/token and the profile is sync-dominated (GPU-bound), so per-submit
  overhead is a real decode cost. Passes-per-submit was a fixed const (FLUSH_EVERY=32). Bigger = fewer
  submits = less overhead, but longer per-submit (shared-GPU display can stutter) — a tunable tradeoff.
- BUILT:
  * flush_every now a tunable AtomicU32 (was const FLUSH_EVERY) + set_flush_every/flush_every/
    flush_every_pinned + LOGSCOPE_FLUSH_EVERY override (a manual pin always beats the tuner). Default 32.
  * SUBMIT_PROFILES = [16,32,64,128,256] (the matrix). begin_frame/end_frame made pub (gate + backend can
    frame explicitly).
  * SubmitTuner (pure): sweeps the matrix on LIVE tokens, `per_cand` samples each, median, settles on the
    fastest, then holds. current()/record()/results()/settled(). Safe to adapt mid-stream because batching
    is bit-exact (changes submit grouping, not output).
  * store::flush_get/flush_put (submit-flush.tsv, keyed by adapter) — settled value cached per GPU.
  * WIRED in backend.rs: Engine holds an Option<SubmitTuner>; at load → pinned/cached/adapt (logged). Each
    reply (≥8 tokens) feeds ONE sample = mean sec/decoded-token (length-normalized), advancing the sweep
    across replies; on settle it caches + logs the sweep table. Hooked after the per-turn profile line
    (NLL releases the decode closures' &self.gpu borrow there, so self.submit_tuner is free to borrow).
- LOGGING (M2-visible): at load "submit batch = adapting…/cached/override"; per reply "submit-tuner
  sampled flush_every=X → Y ms/tok (i/5 candidates)"; on settle "SETTLED flush_every=Z (cached)" + the
  per-candidate ms/tok sweep table.
- VALIDATED IN-SANDBOX (--gate-submit, all ✓): SubmitTuner sweep/settle/hold + empty→default; flush_every
  controls submit count for the same framed work (8 submits at flush=4 vs 1 at flush=64); a framed
  DEPENDENT chain is BIT-EXACT across flush (max|Δ|=0) — adapting never changes output; set_flush_every
  clamps+reports. Full kernel suite + perf/autotune-cache/gemm-spec/q8direct gates still green (the flush
  change touches every framed path; begin/end_frame now pub). Tile/flush QUALITY is M2-measured.
- M2 USE: fast manual signal — LOGSCOPE_FLUSH_EVERY=64 (then 128, 256) on the 14B, watch submits/tok +
  tok/s in the profile line. Hands-off — let the tuner sweep over ~10 replies (5×2) and cache. Decode
  lever; complements the prefill GEMM-tile lever.

## Behavioral-directive fan-out (:fuzzd) + Qwen early-stop diagnostics — 2026-06-12
- FLUSH_EVERY=64 WIN (M2, user-confirmed): Qwen-14B decode 1.02→2.44 tok/s (26→13 submits/tok), prefill
  241→121s — both ~halved purely from batching more passes/submit. Decode WAS submit-overhead-bound.
  Decode tile=1 confirmed best for every shape on both models (flush_every is the decode knob, not tile).
  Next levers: LOGSCOPE_FLUSH_EVERY=128/256 (more batching), then LOGSCOPE_PREFILL_TILED=1 (prefill compute).
- DIRECTIVE LIBRARY (directives.rs, pure, 8 standalone tests ✓): observation — a model's behaviour shifts
  a lot with its instructions, so a fan-out that varies the DIRECTIVE (strategy/process/constraint/persona/
  risk) explores genuinely different approaches, not N rewordings. LIBRARY = 20 directives across 5 tags.
  select(n,seed) picks n distinct, spread across tags, deterministic. fanout_prompts (system-base compose)
  + fanout_user (user-turn compose, for the queue). WIRED: new `:fuzzd N` / `:fd N` command (vimmode
  Command::FuzzDirective + parser) mirrors `:fuzz` but fans directive-perturbed branches via
  directives::fanout_user → queue → one branch each (temperature still varies within). `:fuzz` (lexical)
  kept, so the two modes sit side by side to compare. NOTE: directive currently rides in the USER turn
  (the queue varies the user message; system prompt is built separately) — a later refinement is a
  per-branch SYSTEM override + labelling each branch by directive name in the spatial view.
- QWEN EARLY-STOP DIAGNOSTICS (backend.rs): Qwen-14B produces 9/2/2 tokens then stops; Llama-3.2 gives a
  full 194-tok reply, so UI/decode/sampling are fine — it's the Qwen/ChatML path. Verified in-code: the
  ChatML prompt assembly is textbook, and the tokenizer registers control tokens (type 3/4) as verbatim
  specials (<|im_end|> resolves, template→ChatML), so the prompt encodes correctly — NOT an obvious
  assembly bug. Can't reproduce the Qwen tokenizer path in-sandbox, so added a SHORT-REPLY diagnostic
  (fires when produced<24): captures the stop token from the decode callback and logs the stop reason
  (model emitted EOS / stop token / cap / guard), the stop set, the next-token argmax, the decoded text,
  and the last ≤16 token IDs. The next M2 run will show whether Qwen emits 151645/151643 early and what
  the generated ids actually are (e.g. whether content preceded the visible " for clarity" fragment).
- VALIDATED: directives 8/8 standalone; vimmode parser (incl. :fuzzd) + chat dispatch + backend
  diagnostic all compile under cargo check --features wgpu-engine. Diagnostic output + directive QUALITY
  are M2-observed (need a real Qwen run / real fan-out).

## ROOT-CAUSED + FIXED the Qwen early-stop: special-token smuggling from injected content — 2026-06-12
- DIAGNOSTIC PAYOFF: the SHORT-REPLY logging pinned it. Qwen produced 6 tokens, decoded text
  "<|im_start|>//, and logs.", generated ids [151644, 322, 11, 323, 18422, 13]. The FIRST generated
  token is 151644 = <|im_start|> — the model emits its OWN ChatML turn-opener, then a fragment, and the
  runaway guard cuts it (stop reason was "no stop token seen", next-token argmax 151645 = <|im_end|>).
  That's the signature of a CLOSED-looking turn structure: the model thinks it must open the next turn.
- ROOT CAUSE: the prompt is assembled as ONE string (template markers + injected content) and then
  encode()'d, and encode() matches special-token strings ANYWHERE. full_system() injects a log_block —
  the open log's last lines + repeated patterns, verbatim. logscope inspects logs, including LLM logs, so
  a log line containing "<|im_end|>" becomes real control token 151645 mid-system-prompt → the system
  turn "closes" early → structure corrupt → model emits <|im_start|>. Explains the Llama/Qwen split
  exactly: "<|im_end|>" is a ChatML special but NOT a Llama-3 one, so the same log broke Qwen, not Llama.
- FIX (chat.rs neutralize_markers): all INJECTED CONTENT (system prompt incl. log_block, every user/
  assistant turn, the continue-prompt partial) is passed through neutralize_markers before going into the
  template. It inserts a zero-width space after the leading '<' of any "<|"/"<｜" sigil — invisible in the
  UI, read as ordinary text by the model, and no longer matching the special. The builders' OWN
  structural markers are written as literals and never neutralized, so they still become the right control
  tokens. No-op for normal text (only fires when content already contains a sigil). Applied in all three
  builders (chatml/llama3/plain) + build_continue_prompt; neutralizing the assistant partial also stops a
  once-leaked "<|im_start|>" from re-injecting and compounding across turns.
- VERIFY DIAGNOSTIC (backend.rs): added a one-line prompt scan logged each generation — count + positions
  of special tokens in the ENCODED prompt + the tail ids. A correct ChatML prompt now shows exactly
  2·turns+1 specials and a tail ending on the assistant primer; the NEXT run confirms the stray-special
  count dropped to the structural minimum (fix worked) or, if not, shows the real prompt structure.
- VALIDATED: neutralize logic standalone (markers broken, text readable, clean text untouched, fullwidth
  sigil handled) + a chat.rs unit test; full app compiles under cargo check --features wgpu-engine. The
  end-to-end Qwen reply is the M2 confirmation. A more principled future fix is piece-wise encoding
  (encode_ordinary for content, structural marker ids spliced in) so content is *never* special-matched.

## Chat import (:open) + frontier fan-out (:continue-all) + spatial-chat mockup — 2026-06-12
- WHY: the branching workflow's payoff is leaving a model to expand a SPACE of branches overnight, then
  letting an agent surface the region that compiled+tested best. Two new commands + a visual mockup of the
  whole loop (the egui app can't be screenshotted from the sandbox — no display/GPU — so the mockup is how
  you SEE it now; the Rust makes it real on the M2).
- :o | :open <path> (vimmode Command::Open + parser): imports a chat in the markdown :export shape
  (## You / ## Assistant sections, optional # title) back into the conversation. parse_imported_chat is
  pure (case-insensitive headers, you/user + assistant/asst, keeps fenced code, drops unknown sections,
  empty→no clobber). load_imported checkpoints first (undoable), rebuilds the linear msg tree, reseats the
  tip. examples/sort-ints.chat.md is a ready seed to import.
- :ca | :continue-all (vimmode Command::ContinueAll): enumerates every branch tip (leaf_ids — ids that are
  no other msg's effective parent, same rule as the convo-graph) and queues a continuation on each. Drains
  serially through the existing generation queue via a CONTINUE_ALL_TOK sentinel → expand_leaf(leaf):
  sets the lineage head to that leaf, builds root→leaf + assistant primer, appends a fresh assistant turn,
  streams. So the whole frontier advances one turn per call; run it repeatedly (+ :fuzzd for width) and the
  space grows. Width is fuzz/reroll; continue-all is the "advance everything" pump.
- VALIDATED: parse_imported_chat + leaf_ids logic standalone (3-turn parse w/ code block preserved,
  unknown-header drop, branch-tip enumeration incl. 3-way fork); vimmode parser tests for :open/:o and
  :ca/:continue-all; full app compiles under cargo check --features wgpu-engine. Live behavior (actual
  overnight expansion, the summary agent) is M2/real-run territory.
- MOCKUP (mocks/spatial_chat_mock.html, also presented standalone): self-contained, deterministic
  (mulberry32 integer seed → bit-reproducible space). Click ":open sort-ints.chat" → seed convo; ":fuzzd
  focus" fans directive-varied branches; ":continue-all" / "leave overnight (×6)" grows the tree (bounded
  ~240 nodes, organic 0/1/2-way branching); each assistant node gets a mock cargo-check+test verdict
  (green=compiles+tests, amber=compiles/tests-fail, red=build-fails) with lineage bias so coherent good/bad
  regions emerge; "/" fuzzy-finds nodes; "🤖 summarize best region" ranks every subtree (≥3 verified) by
  pass-ratio, highlights the winner (◆ + purple), and writes the agent's write-up — the compiler is the
  grader, not the model. Pan/drag, scroll-zoom, j/k move focus. Mirrors the real app's palette + vim cmdbar.
- HONEST: the mockup's dynamics + verdicts are invented to make the LOOP legible; real branch text comes
  from the llama model and real verdicts from the sandbox `cargo check` oracle once run on the M2. The
  "summary agent" here is the SELECTION (best-region by verify) + a templated write-up; wiring it to
  actually prompt a small model with the region is the thin next step.

## While-you're-out: model recommendations + best-region selection (:best) — 2026-06-12
- MODEL DOC (docs/07-model-recommendations.md): grounded in what the engine actually runs (llama-arch +
  Qwen2 + ChatML/Llama-3/Plain) and the current (mid-2026) landscape (searched). Support matrix:
  * WORKS TODAY: llama-3.2-1b (breadth), Qwen2.5-Coder-14B (depth, post-fix).
  * EASY ADDS (same arch/template, ~drop-in): all Qwen2.5(-Coder) sizes, Llama-3.1/3.3-8B,
    DeepSeek-R1-Distill (Llama/Qwen2). First three to try: Qwen2.5-Coder-3B, Llama-3.3-8B,
    DeepSeek-R1-Distill-Qwen-7B.
  * NEEDS ARCH WORK, ranked: Qwen3-Coder-30B-A3B (MoE) is the HIGHEST-VALUE target — 30B quality at ~3B
    active params, >20 tok/s on M-series, a near-perfect fit for the "small effective compute + structure"
    thesis; work = the MoE layer (router top-k + expert FFN load/address), RoPE/attn reuse Qwen. Then
    dense Qwen3, Gemma 2/3 (gelu + logit softcap + dual norm), Phi-3/4, Mistral/Devstral ([INST] + sliding
    window). Two-tier setup recommended: small coder for breadth fan-out, 14B (→ MoE-30B) for depth.
- BEST-REGION (bestregion.rs, pure, 5 standalone tests ✓): the real, tested version of the mockup's
  summary-agent SELECTION. best_region(parents, verdict, min_verified, min_ratio) → the subtree with the
  most verified-good nodes (gated by ratio, tie → ratio → depth → root), and a region must START at a
  verified node (the user-turn root never wins). Verdict enum maps onto the sandbox classify() output.
  Validated: picks the deep-green line, rejects when nothing verified, ratio gate excludes mostly-red,
  prefers a substantial mostly-green region over a tiny perfect one, leaf enumeration.
- :best COMMAND (vimmode Command::Best + chat wiring): builds effective_parents + per-node verdicts, calls
  best_region, highlights the winning subtree (purple rings in the spatial view, new `region` field) and
  focuses its root. HONEST PROXY: node_verdicts currently scores "assistant turn produced a fenced code
  block" = Pass (so `:best` = largest code-PRODUCING region). Swapping in the per-node `cargo check` oracle
  turns the SAME selection into largest mostly-COMPILING region — no change to bestregion. That oracle
  wiring (verify-all over leaves, bounded, populating a per-node verdict store) is the documented next step
  and the thing I held off doing blind (it spawns cargo processes I can't exercise here).
- VALIDATED: bestregion 5/5 + parse_imported_chat/leaf_ids logic standalone; vimmode parser incl. :best;
  full app compiles under cargo check --features wgpu-engine. Engine untouched this session (gates
  unchanged). The KV-cache prefix tree (the depth-search perf lever you asked about) I deliberately did
  NOT implement blind — its correctness is bit-exact-gateable but the KV fork/evict + the actual speedup
  want your hardware in the loop; docs/07 + the prior notes leave it scoped and ready to greenlight.

## Modal navigation redesign — make Esc + modes VISIBLE and USABLE — 2026-06-12
Reported: "the escape and modal shuff still wasnt visible or usable last time i tested the gui."
ROOT CAUSES found in the code:
  1. NOT VISIBLE — both handle_modal (key handling) and the mode indicator (status_bar) were gated
     `if self.spatial_view`, so in the DEFAULT linear view there was no modal and no indicator at all;
     and even in spatial the indicator was an 18px strip pinned BELOW the composer, easy to miss/clip.
  2. NOT USABLE — entering spatial didn't force a known mode, so you could land in a focus state where
     the composer ate keys; nothing surrendered composer focus on the view switch.
FIXES (compile-clean under cargo check --features wgpu-engine; interaction FEEL still M2-pending — the
GUI can't run in-sandbox):
  - PROMINENT ALWAYS-VISIBLE INDICATOR: new `modal_banner` at the TOP of the chat area (both views) — a
    colour-coded mode pill (NORMAL cyan / INSERT green / COMMAND amber / SEARCH violet, from the new
    Mode::rgb), then either the live `:`/`/` input or a contextual key hint + `turn n/N` + a `?` button.
    Replaces the buried 18px bottom strip (status_bar deleted).
  - MODAL IN BOTH VIEWS, SAFE BY DEFAULT: handle_modal now runs in linear too, but the mode now DEFAULTS
    to Insert so plain typing is never blocked. Switching INTO spatial enters Normal + surrenders composer
    focus (motions + indicator immediately live); switching INTO linear enters Insert (typing live). All
    three view-switch sites (top-bar toggle, :linear/:spatial, coordinator Aggressive/Barebones) route
    through one `set_view` helper.
  - ROBUST KEY HANDLING: Esc is now the universal back-out (closes the `?` overlay → drops any typing
    sub-mode to Normal + surrenders focus + clears pending → cancels a pending z/leader). Added a guard:
    if some OTHER text field (a side panel) holds focus, Normal-mode capture bails so it can't steal keys.
    Normal still drains Text events before the box is built (what actually stops motion keys from typing).
  - DISCOVERABILITY: `?` (and the banner's ? button) toggles a grouped keybinding overlay (MOVE / CAMERA
    [spatial only] / ACT); the inline hint is view-aware (camera keys only shown in spatial).
  - BUGFIX found en route: `:o` was claimed by BOTH :overview and :open; the first match (overview) won,
    so `:o <path>` never opened. Overview now keeps :overview/:fit (and the `o` key); `:o` is open.
MOTION MAP (firmed up): j/k focus along the live branch · h/l switch sibling branch (makes it live) ·
  g/G first/last · a or Enter write (Insert) · b branch · / find · space-/ tree search · space-\ fuzzy
  tree · : command · ? help · Esc back out. Spatial adds: i/I zoom · z+h/l yaw · z+k/j pitch · o refit.
TESTS: vimmode 8/8 (added: `?`→Help, Mode::rgb distinct-per-mode, is_typing, and the :o/:open fix);
  bestregion 5/5 unchanged. The focus/capture behaviour is logically sound but UNVERIFIED on hardware —
  please test on the M2: (a) does the top pill show + change colour, (b) does Esc reliably enter NORMAL
  from the composer, (c) do j/k/h/l move focus in both views, (d) does `:`/`/`/`?` work.

## KV-cache PREFIX TREE — the depth-search perf lever, built — 2026-06-12
The single warm-resume anchor reused the LAST turn's KV when the next prompt shared its prefix — perfect
for linear chat, useless for branches: a sibling switch or N `:continue-all` continuations off one leaf
share a prefix with a DIFFERENT lineage than "the last turn", so the engine re-prefilled from scratch each
time. Generalized the single anchor into a small POOL of recent branch tips; on each prompt the engine
reuses the entry sharing the longest usable prefix. The risky KV-copy machinery already existed
(seed_from_prefix + batched-suffix verify_forward) and is bit-exactness-checked, so this is mostly the
selection decision + pool bookkeeping.

PAYOFF: fanning N continuations off one leaf becomes **1 prefill + N decodes** (the first pays the leaf
prefill and caches it; the rest seed from cache). Sibling switches reuse a previously-visited branch's KV.

- kvtree.rs (NEW, pure, 6 standalone tests ✓): `pick_anchor(entries, prompt, min_prefix, max_suffix)` →
  the (index, reuse_len) of the cached entry with the longest usable shared prefix, same thresholds the
  single anchor used (≥16 shared, ≤256 divergent), clamped to each entry's batch-prefilled rows + filled
  pos + prompt.len()-1. Ties → most-recent (warmest). Tests cover: longest-branch pick, the fan-out case
  (2nd continuation off a leaf still reuses it via the leaf+gen0 entry), threshold rejection, empty pool,
  tie→recent.
- backend.rs: `anchor: Option<…>` → `anchors: Vec<…>` + `kv_cap`. Selection now `kvtree::pick_anchor`
  over the pool; reuse seeds from `anchors[idx]`; after a turn the lineage is pushed and the oldest evicted
  past cap; warm-resume (continue_last) pops/pushes the most-recent entry. ENV TOGGLE: `LOGSCOPE_KV_TREE`
  unset → cap 1 = the original single-anchor behaviour, byte-for-byte unchanged; =N keeps N tips warm
  (each holds a full KV cache → trades memory for fewer prefills; on the 64GB M2 a cap of 4–8 is cheap).
- gpu-gemm --gate-kvtree (NEW, runs on lavapipe): proves reusing a cached lineage's KV for a DIFFERENT
  branch reproduces a full fresh prefill — sibling (short shared prefix), extension (full shared prefix),
  and exact-prefix (reuse all-but-1, re-prefill 1). Result here: argmax-identical, worst |Δlogit|=1.6e-5
  (within the existing reuse tolerance). So multi-entry pool reuse is correctness-safe; the SPEEDUP is the
  only M2-side unknown.

M2 BENCHMARK PROTOCOL (the part only your hardware can answer):
  1. Baseline vs tree: run the SAME branchy session twice — once with LOGSCOPE_KV_TREE unset, once with
     LOGSCOPE_KV_TREE=6. The engine already logs per turn either "full prefill" or "reused {l} anchor
     tokens + prefilled {suffix}" plus the prefill seconds — that line IS the measurement.
  2. Fan-out (the headline): in spatial view, grow a few leaves, then `:ca` (continue-all) repeatedly (or
     `:fuzzd N` then continue). With the tree ON, the 1st continuation off each leaf logs "full prefill",
     every subsequent one off the same leaf logs "reused …" with a tiny suffix → prefill time should drop
     from ~the full prompt to near-zero for the reused ones. Compare total prefill seconds ON vs OFF.
  3. Sibling switch: branch (b), switch siblings (h/l), continue each — revisiting a branch should log
     "reused …" instead of re-prefilling.
  4. Watch memory: cap×(KV cache). If you see pressure on a huge window, lower the cap. Report back the
     prefill-second deltas + peak mem and we tune the cap / eviction (LRU-on-hit is the next refinement).
VALIDATED: kvtree 6/6 + gate-kvtree (lavapipe, argmax-identical) + gate-prefill still green + app compiles
under cargo check --features wgpu-engine. Engine lib.rs untouched. Correctness is proven in-sandbox; the
prefill-time win is yours to measure.

## M2 feedback round: KV win confirmed, depth-count grammar, KV doc, mock fix — 2026-06-12
- KV REUSE CONFIRMED ON M2 (SmolLM2-360M): full prefill 8.06s (1087 tok) → "reused 1058 anchor tokens +
  prefilled 29 in 1.37s" — ~6× prefill drop; decode rate unchanged (~15 tok/s, as expected — reuse cuts
  prefill, not decode). NOTE: that run had NO "KV prefix tree ON" line → it was at cap 1 (single anchor,
  mostly-linear path). To exercise the TREE specifically: LOGSCOPE_KV_TREE=6 + branch (b) + switch siblings
  (h/l) + continue — revisiting a branch should log "reused …" where cap 1 misses with "full prefill".
- GATE "unknown gate '--gate-kvtree'": the user ran it from a SEPARATE gpu-gemm git checkout that didn't
  receive the new gpu-gemm/src/main.rs (cargo "Finished in 0.02s" = stale binary, source unchanged there).
  The gate IS in the bundle at logscope-suite/crates/gpu-gemm/src/main.rs and passes here (argmax-identical,
  worst |Δlogit| 1.6e-5). Fix on their side: sync that main.rs into the gpu-gemm checkout + rebuild.
- KV CACHE DOC (docs/08-kv-cache.md): what the KV cache is (stored per-layer K/V so attention isn't
  recomputed; reuse cuts prefill), the precise "is it latent mapping" answer (K/V are latent but it's
  EXACT-prefix reuse, no similarity search), and the reshape question: the cache is GPU-resident and
  right-sized per generation (new_cache(n_layers, cap=prompt+reply, kv_dim) — not max-window, not CPU).
  Reshape avenues: (1) PAGED/SHARED KV (vLLM-style blocks; branches share ancestor blocks by reference
  instead of seed_from_prefix COPYING → free forks + less pool memory), (2) int8 KV pool (QKvCache exists,
  4× more warm tips per byte). Both sizeable; watch peak mem vs LOGSCOPE_KV_TREE=N to know when they matter.
- DEPTH-COUNT COMMAND GRAMMAR (vimmode parser, 9 standalone tests ✓; app compiles):
  * Numeric motion count: 21j / 6k / 3l — digit prefix accumulates in Normal mode, repeats the next
    repeatable motion (j/k/h/l/i/I), shown as "N×" in the banner, cleared on Esc/non-motion.
  * :ca [N] / :Nca → ContinueAll(depth): continuation on every leaf, fanned N ROUNDS DEEP. Round 1 fires
    immediately; the queue-drain re-fires fan_step on the NEW frontier for the remaining rounds (true
    depth, not just repeat).
  * :fa [W] ['tmpl'] / :Nfa W 'tmpl' → FuzzyAll{depth,width,template}: depth×width templated fuzzy fan.
    Task = the template (placeholders {lastmsg}→leaf text, {msgid}→leaf id, filled per leaf) or, if blank,
    the composer text. Each round fans `width` fuzzed variants per leaf. e.g. :4fa 10 'redo, not like
    {lastmsg}' = 4 deep × 10 wide.
  * :stop / :st → cancel a running fan + clear the queue. Clear button + resync_tree also drop the fan.
  * SAFETY: fan is hard-capped at MAX_FAN_QUEUE=96 pending so a deep/wide (esp. fuzzy, which is
    exponential without pruning) fan paces itself instead of flooding the device. The proper bound is
    top-K pruning at each level — that's the verify-all oracle (judge → keep the best W) which is still the
    queued next big thread; today's cap is the stopgap.
  HONEST: the parser + count + template fill are standalone-tested; the fan EXECUTION (re-fire-on-drain
  timing, the depth loop on real generations) is compile-checked only — GUI/model can't run in-sandbox.
  Please test :3ca and :2fa 4 'x' first (small) and watch the "fan: +N queued · R rounds left" note before
  trying :6ca / deep fuzzy.
- MOCK FIX: spatial_chat_mock.html threw "find.addEventListener is not a function" — `find` was never
  assigned the element so it resolved to the native window.find. Added `const find =
  document.getElementById('find')`. Fixed in both copies.

## Structured decomposition (:fill/:assemble) + the multi-model reasoning — 2026-06-12
Riffed from: "prompt blending… get an AI to write sub functions you chainload through shorter sidechannels;
large model scaffolds main+loops, small model fills the loops; maybe a hybrid model with more attention
pathways coordinating between models." Sorted into buildable-now / known-technique / research-hard and
built the tractable core.
- KEY INSIGHT (and why it ties to the KV tree): the "shorter sidechannel" isn't a truncated context — it's
  the KV prefix tree making the SHARED context free. Each sub-task embeds the same skeleton (shared prefix
  → prefilled once, cached) and differs only in a short "implement THIS body" tail. So per-sub-task compute
  is short while each sub-task still sees the full skeleton. Best of both; no context/cost trade.
- decompose.rs (NEW, pure, 6 standalone tests ✓): find_holes (todo!()/unimplemented!() + enclosing fn sig,
  heuristic), subtask_prompt (full skeleton as the KV-shared prefix + one body to fill), strip_fence,
  assemble (splice bodies back, right-to-left by offset so a fenced/out-of-order fill still splices).
- :fill [path] / :assemble (chat.rs): :fill takes a skeleton (path, else composer if it has a todo!(),
  else latest reply), fans one sub-task per hole sharing the skeleton prefix (KV-reused), stores a
  FillSession. :assemble maps the last N assistant replies onto the holes positionally and splices into
  the composer for review. Added to the help overlay. Parser 9/9 (incl. :fill/:asm).
- THE ORCHESTRATION CLOSER (documented as next, not built): "keep the green, retry the red" — cargo check
  each spliced fill, re-fan failing holes. This is the SAME verify-all oracle that gives :best real
  verdicts and bounds :fa's width×depth blow-up via top-K pruning. ONE oracle, THREE payoffs — flagged as
  the single highest-leverage next build.
- MULTI-MODEL split (docs/09): coordinate at the TOKEN level now (model A skeleton → model B fill; the
  fabric's run_tokens is already a clean tokens-in/tokens-out hand-off; a router dispatches sub-tasks by
  difficulty, KV tree = shared substrate). Speculative decoding = the token-level large+small dual (small
  drafts, large verifies). True cross-model ATTENTION is research-hard (representation alignment); the real
  "many coordinated pathways" answer is MoE (Qwen3-Coder-30B-A3B, already the top model target). Built the
  token-level piece; left cross-model attention as a training project, not a config.
- VALIDATED: decompose 6/6 + vimmode 9/9; app compiles under cargo check --features wgpu-engine. The :fill
  fan + :assemble splice EXECUTION is compile-only (model can't run in-sandbox) — test with a small
  skeleton (2-3 todo!()s) and watch the sub-tasks share the prefix in the prefill log ("reused … tokens").

## Headless scriptable CLI (logscope-cli) + test-conversation suite — 2026-06-12
Goal (user): generate more test conversations; build a CLI so commands can be crafted here, run once on the
M2, piped to a file, and grown into a regression suite — since the GUI can't run in-sandbox but a
mock-backed headless runner can.
- CRATE SPLIT (lib + bins): logscope-egui is now a library with two bins. main.rs is a thin GUI entry
  (`logscope_egui::app::LogscopeApp`); src/bin/logscope-cli.rs is the headless CLI. This is the long-
  deferred core/bin split, and it's what lets the CLI reuse the real engine backend + the pure command
  modules instead of duplicating them.
- GUI GATED behind a default `gui` feature (eframe/egui/rfd/async-std/tree-sitter made optional). The CLI's
  whole closure is egui-FREE — backend, fabric, genguard, vimmode, consensus, decompose, bestregion,
  kvtree, treeexport, script, runner — so `cargo build --bin logscope-cli --no-default-features` builds
  LEAN (no eframe/vello). This also fixed an in-sandbox DISK-FULL from eframe/vello debug artifacts (~4.6G).
  GUI bin carries `required-features=["gui"]`; `cargo build` still builds the GUI unchanged.
- script.rs (NEW, pure, 2 tests ✓): `.logscope` format — `#`comment, `>`/`say `/bare = user turn, `:`=
  command. parse_script → Vec<Step>.
- runner.rs (NEW, 3 tests ✓): headless conversation-graph runner. Holds the message tree, drives a Backend
  (real wgpu engine OR a deterministic mock) synchronously through the SAME command language
  (vimmode::parse_command → consensus/decompose/bestregion/treeexport). Wired: say, :ca(depth),
  :fa(depth×width, templated), :fill (synchronous decompose→fill→assemble), :assemble, :best, :open, :goto,
  :export. Minimal ChatML/Llama-3/Plain prompt builder over the lineage with marker-neutralization +
  parse_chat_md import (round-trips :export md).
- bin/logscope-cli.rs (NEW): args --mock | --model GGUF, --system, --max, --final. MockBackend = deterministic
  (code-block replies + fill bodies) so the orchestration tests off-GPU. Real path uses
  `backend::make(BackendKind::Wgpu)` (made `make` PUB) under --features wgpu-engine. STDOUT = artifacts
  (:export/:best/:fill) for piping; STDERR = progress notes + final tree.
- decompose.rs: added extract_code_block (pure, tested) — pull the first ```fenced``` block out of a reply
  so a skeleton embedded in a model reply is used cleanly. Wired into BOTH the CLI runner's :fill and the
  GUI's :fill (fixed a real bug: :fill from a reply was splicing into the prose+fence wrapper).
- EXAMPLES (logscope-suite/examples/): 01-sort-generics, 02-lru-fill, 03-fanout-explore, 04-parser-fill
  (.logscope) + lru-skeleton/parser-skeleton (.chat.md seeds) + examples/README. docs/10-cli.md = the
  harness + suite-building workflow.
- VALIDATED IN-SANDBOX (mock, end-to-end via the actual binary): script parse 2/2, runner 3/3, decompose
  7/7. Ran all example scripts: :ca deepens the tree, :fa fans 8→26 nodes (depth2×width3, shows why the
  fan cap + future top-K pruning matters), :open imports 2 turns, :fill finds 3 holes → fans KV-shared
  sub-tasks → assembles a CLEAN spliced file, :best reports verified regions, stdout/stderr split works for
  piping. Lean CLI compiles with --no-default-features (no eframe).
- COMPILE STATUS: lean CLI built ✓. GUI build (cargo build, gui on) is code-correct (the pre-gating full
  build reached vello_cpu — a late graphics dep — before disk-failing, so the lib split + all GUI modules +
  main.rs refs compiled; only the sandbox disk stopped the final link). User should run `cargo build` (GUI)
  + `cargo build --bin logscope-cli --no-default-features [--features wgpu-engine]` and report any break.
- REAL-MODEL EXECUTION is M2-side (model can't run in-sandbox): run e.g.
  `logscope-cli --model <gguf> examples/01-sort-generics.logscope > runs/01.md` and paste back — that closes
  the validation loop for the fan/fill/ca flows that were previously compile-only.

## First real-model run (M4 Max, DeepSeek-R1-Distill-Llama-8B-Q4_K_M) — findings + fixes — 2026-06-12
User ran all 4 example scripts through logscope-cli with the real model and pasted back the captures. The
HARNESS worked end-to-end on real hardware (correct trees, :2ca depth, :fill fan over holes, assembly,
:best, :export, stdout/stderr split). The MODEL output exposed two fixable issues + one validation:
- ISSUE 1 — token budget: 256 truncated EVERY reply mid-reasoning (R1-Distill reasons before answering).
  Bumped CLI default 256 → 512; doc says reasoning models want --max 1024-2048 + --system ''.
- ISSUE 2 — fill spliced the reasoning trace as the body: e.g. assembled `fn new(...) -> Self { Okay, I
  need to implement... <whole essay> }`. The model DID reason to correct code inline
  (`Lru { cap, map: HashMap::new(), order: Vec::new() }`) and returned a clean fenced block for `put` — my
  :fill just took the reply verbatim. FIX: decompose::extract_body (NEW, pure) — strips <think>…</think>,
  takes the LAST fenced block (or the trailing `fn …{ body }`'s inner body), else the de-fenced text. Wired
  into the runner's :fill + :assemble AND the GUI's :assemble. 4 new tests built from the actual R1
  outputs; decompose now 11/11.
- VALIDATION — :best proxy is provably insufficient: it reported "4/4 verified" on code that wouldn't
  compile (`fn sort_vec mut vec {`, `containsKey`, double `&mut vec` borrow, `Vec::new(vec)`). Strongest
  possible motivation for the verify-all `cargo check` oracle (the planned keystone). The decomposition
  THESIS held though: each scoped sub-task got sensible, mostly-correct reasoning even though the model
  couldn't write the whole file — structure made the small model's per-hole job tractable; the gaps were
  budget + extraction, both now addressed.
- NET: re-running 02-lru-fill with --max 1536 --system '' should now assemble real bodies. extract_body is
  unit-validated; the M4 re-run is the confirmation (model can't run in-sandbox).

## Batch runner (gen-runs.sh) + bootstrap for scaled test generation — 2026-06-12
- gen-runs.sh (suite root): batch logscope-cli over a numbered range. Key/value args (no dashes):
  `./gen-runs.sh range 1-4 maxtokens 1536 subdir ./runs/x/ model ~/m.gguf` (also: system, examples, cli,
  final). `range N-M` → runs NN-*.logscope; model=mock|PATH; writes <name>.md (stdout artifacts) +
  <name>.log (stderr notes) per script; prints a script/nodes/seconds table; auto-detects the binary in
  target/{release,debug} or via PATH. Tested in-sandbox with the mock (range 1-4, single, missing-range).
  maxtokens → --max (per-reply generation cap; sets max_tokens+reply_ceiling; NOT n_ctx).
- BOOTSTRAP.md (suite root): a primer to paste into a SEPARATE Claude chat (alongside this repo) so it can
  generate large branching stress tests (thousands of nodes, ~a day). Contains: orient-yourself reading
  list (FIELD_NOTES as the evolution narrative + docs + the 4 grammar-defining source files; code wins over
  prose), the thesis, the .logscope format, the EXACT wired command grammar (the CLI subset; others are
  "skipped"), the branching math (:Dfa W → L·W^D leaves; tables for :3fa8/:4fa6/:5fa5/:4fa8), the runtime
  math + a calibrate-from-gen-runs.sh-output step, what makes a GOOD (not just big) test (multi-hole :fill
  skeletons, {lastmsg} carry-over fa, interleaved :ca, periodic :best/:export checkpoints, --max 1536 +
  --system '' for R1), the deliverable spec, and the gen-runs.sh invocation. KEY: the headless :fa is
  UNCAPPED (GUI caps pending fan at 96; the runner does not) — intentional huge runs work, size them
  deliberately. The "reason about how the project evolved" ask is served by FIELD_NOTES (chronological) +
  git log + docs — which is exactly why FIELD_NOTES is kept as a running log.

## Clarify-first Q/A (:quest / :ans, :qa→stop) — 2026-06-12
Idea (user): a small model that ASKS questions first — builds a Q/A template, uses context from other
branches to offer multiple-choice + open-ended answers — to focus generation and enhance speed. Naming:
user noted :qa reads as vim "quit all" → made :qa a stop alias; :quest N is the question pass.
- quest.rs (NEW, pure, 3 tests ✓): Question{text, kind: Open|Choice(Vec)}; quest_prompt(task,n,context) (ask
  for N ranked clarifying questions, draw options from sibling-branch context); parse_questions (numbered /
  Q:/Q1: / - leads, inline "(a / b / c)" groups, following "Options:" lines); harvest_options (propose MC
  options from off-path branch texts by stem+substring keyword overlap — "sorting"~"quicksort");
  render_template (the Q:/[a][b]/A: fillable block); parse_answers (filled block → (q,a) pairs);
  clarified_context (answers → "Resolved decisions: …" block to prepend to the next gen).
- vimmode: Command::Quest(usize) (:quest [N] / :Nquest, default 5, clamp 1..40), Command::Answer(String)
  (:ans [path]); :qa|:stop|:st → Stop. Parser 9/9.
- runner.rs (CLI, full + mock-tested end-to-end): :quest runs the model OUT OF BAND (no tree nodes), task =
  last user turn on the lineage, context = off-path node texts; parses questions, upgrades open ones to MC
  from cross-branch context, prints the fillable template to stdout. :ans <file> parses the filled block,
  builds the clarified-context block, and generates ONE focused turn carrying the decisions (the speed win:
  one aimed completion vs a fan of mis-aimed ones). Helpers: last_user_text, offpath_texts.
- chat.rs (GUI, minimal arms — compile-pending, mirrors the working :fill arms): :quest enqueues the
  question-prompt (reply = the question list); :ans parses the composer as a filled block → replaces it with
  the clarified-context (ready to send).
- bin/logscope-cli.rs: mock returns a canned question list when the prompt asks for "clarifying questions",
  so the quest→template→ans loop is validated off-GPU.
- VALIDATED IN-SANDBOX (mock): :quest emits the template with MC parsed from "(Vec / HashMap / BTreeMap)" +
  open A: lines, out-of-band (no tree pollution); :ans folds 3 answers into a focused turn (tree shows
  say→ask→resolved→reply). quest 3/3 + vimmode 9/9; CLI builds --no-default-features. GUI arms compile-
  pending (disk; pattern matches working code). docs/10-cli.md updated with the command + a clarify-first
  section.

## Validated a separately-generated stress-test ladder (07–99) — 2 fixes — 2026-06-12
User took BOOTSTRAP.md to another Claude chat; it generated a ladder of stress scripts (07-12 + a 99-
marathon) replaying the REAL unendlich-engine commit spine (r116..r286), correctly flagged its grammar
assumptions as provisional ("the code wins"), and the user uploaded them back for validation. Mock-ran all
through logscope-cli + read runner/vimmode/script. Two real fixes fell out:
- COMMENT BUG (script.rs): `:goto 13   # pickup r133b` failed — :goto parsed its WHOLE arg as a number →
  Unknown → silently skipped, so every pickup in 10/12 (the whole point of 12-pickup-stress) was a no-op.
  Fix: strip_inline_comment in parse_script truncates a trailing `# comment` on command lines, quote-aware
  (a `#` inside a :fa template survives). +1 test; script 3/3.
- SCOPING (runner.rs): :fa/:ca fanned EVERY leaf in the whole tree, so the marathon's "burst then :goto back
  to main line" contained nothing — each :3fa 5 re-fanned the entire accumulated frontier → millions of
  nodes → timeout. Fix: new subtree_leaves(active); :fa/:ca now fan only the active node's subtree.
  RESULT: 10 1142→582, 12 976→196, marathon explode→4602 nodes (completes <90s mock); 07/08/09 unchanged.
- VALIDATION TABLE (predicted / whole-tree / subtree): 07 40/40/40, 08 ~36/28/28, 09 50/50/50,
  10 ~832/1142/582, 11 ~1072/697/697(¹), 12 ~268/976/196, 99 ~6640/explode/4602. (¹ 11+99 :open
  batchwire.chat.md which wasn't uploaded — re-seed skipped.)
- CORRECTED FORMULAS (now in examples/ladder/VALIDATION.md + the math is real): `>`=+2 nodes; :Dfa W over L
  subtree-leaves = L·ΣW^d pairs (×2), new L=L·W^D; :Nca = +L·N NODES (one assistant/leaf/round, NOT pairs —
  why 08 was 28 not 36); :goto M + > = +2-node persistent branch. :goto id mapping: build #k user = node
  2k−1, reply = 2k (:goto 13 → build #7). The marathon's large :goto targets (421/827/1243…) were computed
  from the explosion model → now clamp past end-of-tree → need recomputation (mock + :export ascii +
  reconcile; comments name the intended build so re-pointing is mechanical).
- INTEGRATED: the 7 scripts + the chat's RUN_NOTES.md + my VALIDATION.md live in examples/ladder/. 07/08/09/
  10/12 are self-contained and run as-is; 11/99 need the batchwire seed (not uploaded) or remove the :open.
- OPEN: the GUI's :fa/:ca (chat.rs fan_step) still fan all leaves — CLI now diverges (scoped). Aligning the
  GUI is a follow-up (async fan path, can't build the GUI in-sandbox/disk). Flag for next GUI pass.

## Compile oracle (:check) + speedup timing + KV-disable toggle — 2026-06-12
Built the long-flagged keystone (the cargo/rustc verdict oracle) + the user's measurement asks (a timer +
a way to disable the graph speedups to time the data structure).
- verdict.rs (NEW, pure logic + shells out to a compiler; 4 standalone tests w/ REAL rustc): extract_code
  (concatenate all ```fenced``` blocks, else whole text); compile(code, cmd, file) → bestregion::Verdict
  (writes code to file/temp, runs cmd, Pass on exit 0, Fail otherwise, None if empty/no-toolchain; cmd `{}`
  → path else append for rustc / run-as-is for a project check). DEFAULT_CHECK = rustc --crate-type lib
  --edition 2021 --emit=metadata -o /dev/null (grades self-contained snippets; fragments + cross-crate refs
  correctly fail).
- runner: verdicts Vec parallel to nodes (push keeps it in sync); :check compiles every assistant node's
  code → stores Pass/Fail → prints "# check: P/N compile"; best_report now uses compiled verdicts when any
  exist (tag "[compiled]") else the code-block proxy (tag "[code-block proxy]"). vimmode: Command::Check
  (:check / :chk). GUI chat.rs: minimal arm (note — a synchronous compile would block the UI; oracle is a
  CLI feature).
- TIMER: runner times only the model calls (gen_time/gen_count, Instant around backend.generate); CLI prints
  "# gen: N calls, Xs total, Yms/call avg". gen-runs.sh gained a gen(s) column (separate from wall(s)).
- KV-DISABLE: backend.rs LOGSCOPE_KV_OFF → forces pick_anchor=None (full prefill every turn, no anchor
  reuse at all) + a load-time log line. CLI --no-kv sets it (and clears LOGSCOPE_KV_TREE); gen-runs.sh
  `nokv on` passes it. Workflow: run a branchy script twice (normal vs nokv on), compare gen(s) → the prefix
  tree's contribution. (CAN'T measure in-sandbox — no GPU; the toggle + timer are M4-side.)
- gen-runs.sh: new args `nokv on`, `check 'cmd'`, `checkfile path`; columns now nodes / wall(s) / gen(s).
- VALIDATED IN-SANDBOX (real rustc): verdict 4/4 (good→Pass, bad→Fail, empty→None, extract). :check
  integration on mock (8/8 valid fn bodies compile → :best [compiled] 8/8). THE PROOF — ran the actual
  uploaded outputs through the oracle: 8B broken generic sort → FAIL, 1B `cargo build` bash block → FAIL,
  8B correct LRU new body → PASS (the proxy called all three "verified"). CLI builds --no-default-features.
- NOTE: scripts must include :check (before :best) to grade; --check only configures the checker. The
  real-crate path (--check-file + cargo check -p) is for grading :fill against an actual crate (batchwire).
- DEFERRED (the oracle's downstream payoffs, now unblocked): :fill keep-green/retry-red, :fa top-K pruning
  to bound width×depth by compiling verdict.

## KV-benchmark script + gen-runs subfolder search — 2026-06-12
User's `gen-runs.sh range 6-6` found nothing: 06-batchwire-explore was never uploaded (only the 07-12 + 99
ladder came over; 05/06 + the batchwire seed stayed in the generating chat). Two fixes:
- examples/06-kv-bench.logscope (+ examples/kv-bench-seed.chat.md): purpose-built to MEASURE the prefix
  tree. :open a ~60-line shared Grid module, then :fa 8 (8 variants reuse the module prefix) + :ca 3 (24
  continuations, each reuses its branch's module-rooted prefix) → 42 generations, heavy shared-prefix reuse,
  the prefix tree's best case. The compounding shows with depth (nokv-on re-prefills the whole growing
  lineage each gen; on reuses the parent KV). Run twice (normal vs `nokv on`), compare gen(s).
- gen-runs.sh: numbered-script search now globs `$examples/NN-*` AND `$examples/*/NN-*`, so `range 7-12`
  reaches examples/ladder/ without an explicit `examples examples/ladder`. `range 6-6` → examples/06-kv-bench.
- VALIDATED (mock): range 6-6 → 06-kv-bench 42 nodes (open 2 + fa 8×2 + ca 24); range 7-10 → ladder
  07/08/09/10 resolve from the subfolder. The user's original command now works as-is.

## M2 06-kv-bench log analysis + --kv-tree flag — 2026-06-12
User ran 06-kv-bench (llama-3.2-1b-q4, M2 Max), "taking forever". Log analysis revealed TWO things:
1. The "kv-on" run was actually cap 1 (single anchor) — neither gen-runs.sh nor the CLI set LOGSCOPE_KV_TREE.
   The 8 :fa siblings reused the 640-tok seed (full prefill 7.66s → reused 1.74-1.81s = ~4.2x), because each
   shares the seed with the immediately-prior sibling (cap-1 holds it). But the 24 :ca continuations all
   FULL-PREFILLED (1021-2119 tok, 12-33s each) — cap-1 evicts the 8 branch KVs, so continuing across
   branches finds only the seed shared with the last lineage, and pick_anchor's max_suffix=256 rejects reuse
   when the divergent tail exceeds 256 (the fan replies are 200-1226 tok). The prefix TREE (cap≥8) is exactly
   what keeps all 8 branch tips warm so continuations reuse their parent.
2. DECODE dominates: ~611s decode vs ~178s prefill over the first 17/32 gens (~77% decode). The 1B rambles to
   200-1226 tok/gen at ~14 tok/s (no clean EOS) — runaways at 1226/748/727/709 tok. So capping --max helps
   wall-clock more than KV does; KV only addresses prefill.
FIX: --kv-tree N CLI flag (sets LOGSCOPE_KV_TREE) + gen-runs.sh `kvtree N` arg. Now a proper 3-way:
`nokv on` (off) / nothing (cap 1, single anchor) / `kvtree 8` (the tree). For a clean KV measurement: kvtree
8 + short --max (so prefill is the dominant, consistent variable). Validated: flag flows through (mock).

## Multi-model pool — commands take model classes (:use) — 2026-06-12
User: "when can the commands start taking model classes... like a true multimodel setup". Done. The runner now
OWNS a pool `models: Vec<(String, Box<dyn Backend>)>` + `active_model` instead of taking a single `be` param;
every generation path (gen/say/continue_leaf/branch_say/run_step/run_command, +Fill/Quest/Answer arms) routes
through `self.models[active_model]`. New `:use <name>` command (vimmode Command::Use) moves active_model;
unknown name = no-op that logs available names. CLI: `--model name=path` is repeatable (bare path → "default",
`=mock` → a named MockBackend that tags its replies `[model:<name>]` so switching is visible). Borrow handled
in gen() by a disjoint-field split (params + models don't overlap; NLL ends the borrows before gen_time +=).
- Why it matters: assign each phase to the model that fits — scaffold-big / fill-small / grade-with-rustc
  (examples/05-multimodel.logscope). Each backend keeps its OWN KV anchors, so a :use switch is a full prefill
  on the new model (correct — it hasn't seen the lineage); the 06 prefix-tree reuse happens WITHIN a model's
  run (small's :fill sub-tasks share the skeleton). So switch at phase boundaries, not inside a fan.
- VALIDATED: new runner test use_switches_which_model_runs (2 named mocks, :use routes correctly, bad :use
  stays put) — full lean lib suite 52/52 green. Binary dry-run of 05 + a 4-switch script: [model:architect]/
  [model:worker] tags land on the right turns, unknown :use warns + stays. GUI: added Command::Use(_) arm to
  tabs/chat.rs (single-model note, like :check) to keep its exhaustive match compiling — GUI still
  compile-pending (disk can't fit eframe), but the arm is there.
- Docs: docs/10-cli.md "Model classes" section (pool, :use, the scaffold/fill/grade pattern, per-model KV,
  the memory + speculative-decoding caveats).
- NOT this: token-level draft+verify (speculative decoding) is engine-level, noted as future.
DISK: sandbox hit 100% (40M free) → a SIGBUS on linking; cleared release/other-crate targets + cargo caches
back to 3G. Recurring constraint.

---

## KV cap is not a knob; the 3/24 ceiling was a one-line under-report — and decode is now batched

Three real M2 runs of `06-kv-bench` (llama-3.2-1B, maxtokens 200) settled the KV-tree question. **cap-8, kvtree-8, and cap-16 are byte-for-byte identical**: 7/7 fan reuse, but only **3/24 continuation reuse**, ~369–379s prefill + ~440s decode = ~820s. Raising the pool did nothing, because eviction is FIFO and cap-8 already holds all eight fan tips — capacity was never the constraint.

The real ceiling was in `backend.rs`: a reused turn stored `prefill_len = if reuse { l }` — only the *copied* prefix (640), even though the divergent suffix is then batch-prefilled by `verify_forward` (the same path a full prefill takes), so the **whole** prompt is valid KV. A child trying to resume from that tip got clamped to 640, leaving a 322-token suffix past the 256 reuse window → rejected → full prefill. That, not pool size, is the 3/24 wall. Fixed to `prefill_len = prompt_ids.len()` (both paths leave the entire prompt prefilled; re-seeding it is bit-exact). Expect continuations to start showing "reused" on the next run — gated code, so verify on your build.

**Decode is the floor KV can't touch (~440s = 32 gens × 200 tok × ~14 tok/s), and the harness was making it worse by decoding the fan one branch at a time.** The engine already had `run_batched`/`decode_batched` (continuous batching) and `speculative_generate`, all proven bit-exact in docs/03 — the CLI just never called them, because `Backend` exposed only single-sequence `generate()`. Now wired:
- `Backend::generate_batched(prompts, params, cancel) -> Vec<String>` on the trait, default = sequential loop (so the mock and every impl inherit correct behavior).
- `Engine::generate_batched` (the wgpu path) finds the branches' longest common token prefix, prefills it once, queues each divergent suffix into `run_batched`, and decodes all sessions together — a batch-1 decode barely uses the GPU, so each extra branch is close to free.
- The runner's `:fa` and `:ca` now collect the whole round's branches and make **one** batched call instead of looping.

Verified in-sandbox: lean CLI builds; `06-kv-bench` via mock still yields 42 nodes and `:2fa 2` yields 14 (batched default == sequential); 53 lib tests pass incl. `batched_fan_attaches_every_reply`; **and `cargo check --features wgpu-engine` type-checks the real `run_batched` path against the engine**. Correctness of the batched path itself rests on the engine's existing batched==per-request tests (`0.0` in docs/03). Caveat: batched == sequential is bit-exact only at temperature 0 — at temp>0 each session's RNG (`0x1000+id`) differs, so tokens differ though the distribution matches. Perf (the fan-throughput win) is hardware-only: build `logscope-cli` with `--features wgpu-engine` and re-run `06-kv-bench` with `kvtree 8 maxtokens 200`.

---

## Batched decode on hardware: a fan win, a continuation wash — and `:mergectx`, the pick primitive

First M2 run of the batched path (06-kv-bench, llama-3.2-1B, maxtokens 200) confirms it fires — four
`BATCHED — 8 prompts, shared prefix 640 tok` rounds — but the numbers are honest about its shape. The
**fan** round decoded 8 branches in **98.8s** (vs ~130s sequential — a real ~25% win, the branches
share the 640 seed and have short divergent tails). The **continuation** rounds *grew*: 204s, then
312.8s. The cause is structural: `run_batched` seeds only the *common* prefix across all eight branches
— which after the fan is just the 640-token seed — then sequentially re-prefills each branch's entire
post-seed history, which lengthens every round. The single-sequence path doesn't do that: it reuses
each branch's own tip from the anchor pool (especially now that `prefill_len` advertises the whole
prompt). So batched decode is a win where branches share a long prefix and diverge late (the fan), and
a wash-to-loss for deep continuations. Takeaway for a later pass: keep `:fa` batched; either route
`:ca` back through per-branch anchor reuse, or teach the batched path per-request prefix caches. (Not
changed yet — flagging it.) Also note: the batched fan pushes all user turns then all replies, so node
*indices* — hence short ids and `-N` refs — differ from a sequential fan; consistent within a run.

### `:mergectx` — flatten a shortlist for a bigger model to judge

The selection half of "fan out cheap, pick with a big model." `:mergectx <refs> ['instruction']`
resolves the referenced nodes, lays them out as a numbered candidate list **without** their branching
ancestry (flat — the picker compares replies, not threads), appends an instruction, and generates with
the *active* model. Pattern: `:use small` → `:fa N …` → `:use big` → `:mergectx <the N leaves>`.

Nodes are addressed by a stable 6-char base36 short id (in `vimmode::short_id`, the GUI and CLI compute
it the same way), resolved git-style by any unique prefix, plus `-K` (Kth from the end) and bare 1-based
numbers — all in pure, tested `vimmode::resolve_ref`/`resolve_refs`. CLI: `Command::MergeCtx` + the
runner arm (build merged context → generate). GUI (compile-pending on the engine build): each turn now
shows its id highlighted in the bubble header; typing an id prefix after `:mergectx`/`:goto` offers
clickable completion chips; and `:mergectx` stages the merged candidate text in the composer to send
against any model. Verified in-sandbox: 55 lib tests (+`short_ids_*`, +`parses_mergectx`); example
`07-pick.logscope` via the mock merges 6 fan leaves and generates a pick (16 nodes).

---

## A zsh-style command palette in the GUI composer

Typing `:` in the GUI composer now opens a completion grid of matching commands, plus a hint line for
the top choice. The matching is pure and shared (so it's tested in-sandbox even though the grid is
egui): `vimmode::command_catalog()` is the single source of truth — each `CmdSpec` carries a canonical
name, aliases, an argument signature, and a one-line description. `command_completions(input)` returns
the commands whose name/alias extends the typed prefix (after stripping a leading repeat-count like
`:6`), ranked exact-match → name-prefix → shorter → alphabetical, so `:fu` surfaces `fuzz` rather than
`fa` (whose alias `fuzzy-all` also prefix-matches). It returns empty once an argument is being typed;
`command_hint(input)` keeps returning the chosen command through argument entry, so the description and
arg signature stay visible. The grid (`egui::Grid`, 4 columns) renders below the hint; clicking a chip
fills `:<name> ` into the composer. It slots in alongside last segment's id autocomplete: a `:` command
shows the palette; an id argument (in `:mergectx`/`:goto`) shows id chips; an empty composer shows the
conversation starters. Validated by `command_palette_completes_and_hints` (56 lib tests now); the egui
rendering is compile-pending on the GUI build as usual.

---

## Selectable app themes (research-journal palettes, not Folding@home, not Solarized)

The look was hard-coded as 19 `const Color32` from the unendlich-dash dark UI, used at 281 call sites.
To make themes switchable I turned the palette into runtime state without a risky rename: a `Palette`
(a Copy struct of every themeable color + a `dark` flag + a name) lives in a thread-local, and the old
const names are now accessor functions generated by a macro — `theme::INK()`, `theme::AMBER()`, etc. —
that read the active palette. Converting the call sites was therefore purely additive (append `()`), and
there were no const-context or `match` uses to trip on, so the 281-site change is mechanical and safe.
`install()` now starts from `Visuals::dark()` or `light()` per the palette and re-skins egui's own
widgets; `install_palette(ctx, p)` swaps + re-skins in one call. A combo in the header bar live-switches.

Four palettes (`theme::PALETTES`): the original **Folding@home** (default), plus three sitting in the
requested mid-band — not paper-bright, not near-black, deliberately not Solarized:
- **Reading Room** — light-mid dimmed parchment (#d8d1bf), warm brown-grey ink, muted ochre/ink-blue/
  sage/brick accents. The only light theme; `install` switches egui to its light base for it.
- **Archive** — warm dark-mid charcoal (#2f2b24), clearly lighter than a typical #111 web UI, cream ink,
  brass/olive/dusty-blue accents.
- **Slate** — cool dark-mid blue-grey (#34383f), soft ink, restrained clay/teal/indigo accents.

Validation: `theme.rs` was checked against **egui 0.34 in a standalone crate** (rc=0) — the macro, the
accessors, the rewritten widget kit, and the dark/light `install` all type-check; the picker mirrors the
existing `ComboBox::from_id_salt` usage. The 281-site append is type-preserving (same `Color32`), and the
lean CLI build is unaffected (theme + the touched files are all `#[cfg(feature="gui")]`). The picker and
the converted tabs are compile-pending on the eframe build as usual. Not done yet: persistence — the
choice is per-session (resets to default on restart); an `eframe` storage hook would fix that.

---

## Reading Room gets dirt; the chat composer + a context-card strip

Three GUI changes (all compile-pending on the eframe build; `theme.rs` itself re-checked against egui
0.34, rc=0):

**Reading Room — darker + textured dirt.** Pushed the palette from the bright dimmed-parchment down to a
soiled khaki (bg #d8d1bf → #bdb094, ink a touch darker for contrast on the deeper ground) and added a
`texture: bool` to `Palette`. `theme::paint_grain(painter, rect, ink)` lays a deterministic procedural
texture over a rect — a couple thousand faint hashed speckles plus a few larger soft stains, seeded by
the rect so it doesn't shimmer — and the app paints it behind the central content whenever
`theme::textured()` (only Reading Room, for now). Opaque cards sit on top, so the grain reads through the
gaps as aged/dirty paper. The other three palettes set `texture: false` and are unchanged.

**Bigger message box.** The composer was `desired_rows(2)` with the default body font — the "very small
and hard to read" complaint. Now `desired_rows(6)` with a 14px monospace face; it still auto-grows and
the conversation scrolls above it.

**Context cards next to the chat.** The graph already lives in the right `SidePanel("chat_right")`; I
added a small always-visible card strip at the top of that panel (above the Files/Graph/Context/Prompting
tabs) that floats three quick references drawn from the live conversation: the loaded file name, the
freshest fenced code block (first six lines, with a copy button, via `verdict::extract_code`), and up to
six salient keywords as chips (via `convograph::keywords`). It sits beside the chat regardless of which
subpage is open.

Not done this pass: real per-session→persisted theme choice, and a height-adjustable/collapsible composer
if 6 rows turns out to crowd the reading area (the message list and composer share the central column's
vertical space, so that's the one knob still worth a follow-up). The grain palette + helper are validated;
the app paint call and the chat composer/card edits are compile-pending on your GUI build.

---

## `:strategy` / `:birdseye` — the summarizer-worker counterpart to `:mergectx`

Added a synthesis command (aliases `:bird` `:birdseye` `:strat` `:strategy`) that unifies a branchy
exploration into one condensed bird's-eye view — the inverse of `:mergectx`'s "pick the best." The
summarizer-worker prompt is `vimmode::STRATEGIST_PROMPT`: it tells the model to produce a through-line,
the distinct threads (merging duplicates that recur across branches), what's settled, what's open/
contested, and a single recommended next step — compressing hard, noting dead ends in a clause, and
refusing to manufacture coherence from thin material. `parse_strategy_args` handles the optional
comma-separated ids (default = the whole tree) and an optional quoted focus, including a bare quoted
focus with no ids. The runner arm flattens the selected turns (each capped at 600 chars) into one
transcript behind the prompt, generates with the active model, and attaches the digest at root so it
opens a fresh, smaller space. The GUI arm stages the same digest in the composer. It's in the command
palette under `strategy` (with the aliases), so the new zsh-grid surfaces it.

Validated: 57 lib tests (+`parses_strategy_aliases_and_args`, covering all four aliases, refs+focus, and
the palette); the mock run `:fa 3` then `:birdseye 'which option…'` condenses 8 turns into a synthesis
node (10-node tree). Pattern to use on hardware: `:use small` → wide `:fa` → `:use big` → `:strategy`.

---

## `:segment` / `:blocks` — a graph-enriching block-map (segmenter worker)

A third synthesis command, alongside `:mergectx` (pick) and `:strategy` (unify): `:segment` (aliases
`:blocks`, `:seg`) maps a messy linear chat into a few coherent blocks and names the seams between them,
so the graph can split/branch rather than stay a flat spine. The worker prompt `vimmode::SEGMENTER_PROMPT`
opens a new block only at a natural seam (new topic, intent change, question→answer, correction,
digression, or a degenerate loop gathered into ONE labelled block), and emits per-block metadata built for
the graph: a relation tag (new-topic / continues / refines / feedback-on / correction-of / digression /
loop), 3-6 keywords (feeding convograph's overlap signal), a one-line gist, the boundary signal, and a
health flag — plus a closing "seams & graph notes" section with the strongest split points and any loops
to collapse. The runner flattens turns with `[#n role · id]` markers so spans can be cited; arg parsing
(ids default to the whole tree, optional quoted focus) reuses the strategy parser. Validated: 58 lib tests
(+`parses_segment_aliases`); mock `:fa 2` then `:blocks` maps 6 turns. CLI run-validated; GUI arm compile-
pending. Tested in passing on a real degenerate small-model export (looping numbered questions + a
topic pivot) — exactly the input the block-map is meant to tame.

---

## Ambience pass — baked GI lighting + boreal aurora + high-res grain

Rebuilt the texture layer into a single baked **ambience** so the UI can carry real atmosphere without
per-frame shape spam. `Palette` gained `lit`, `aurora`, and a `light` color alongside `texture`. One
function, `paint_ambience(ctx, painter, rect)`, bakes all enabled layers for the active palette into a
single `ColorImage` → `TextureHandle`, cached by (device-pixel size, palette name) and drawn as ONE quad
per frame; it rebuilds only on resize or theme switch (size coarsened to a 24-px grid so dragging doesn't
thrash, long side capped at 2048 texels — raise for crisper grain at more rebuild cost). app.rs now calls
`has_ambience()` + `paint_ambience(ui.ctx(), …)` in place of the old circle-based `paint_grain`.

The three baked layers, composited bottom→top with a premultiplied `over`:
- **Global illumination** — a soft light pool from upper-centre (smoothstep falloff) plus an edge vignette
  that sinks corners toward 45% of the bg. Warm light on the paper themes (a page under a lamp), cool on
  the dark ones.
- **Boreal aurora** — four wavy vertical curtains in the palette's cyan/green/violet, hung in the upper
  field by a vertical envelope, drifting left/right by per-row sine. Subtle, behind the cards.
- **High-resolution grain** — two deterministic octaves (dense single-texel + medium 2×2) in the ink
  colour, so it reads as fine film/paper grain at device resolution rather than coarse dots.

Per-palette: Reading Room (warm GI + dirt), Archive (brass GI + grain), Folding@home (subtle cool GI +
fine grain), Slate stays clean (all off, the no-atmosphere option). New **Boreal** palette — deep
midnight-sky near-black, luminous teal accent — turns on the full set (GI + vignette + star-grain +
aurora) as the showcase. Picker now lists five.

Validated: theme.rs type-checks clean against egui 0.34 in the standalone scratch crate (forced recompile,
rc=0; `ColorImage::from_rgba_unmultiplied`, `load_texture`, `TextureOptions::LINEAR`, `painter.image`,
`pixels_per_point`, the `RefCell` ambience cache all compile). Lean CLI build + 58 lib tests unaffected
(theme/app are gui-gated). VISUAL TUNING IS HARDWARE-PENDING — the alphas (GI 0.12, vignette 0.18, aurora
0.075, grain 0.03–0.15), the light origin, aurora count/width, and the 2048 cap are first-guess values to
adjust once it's on a real screen.

---

## Ambience v2 — dropped Boreal; an old-dusty-library scene with cursor/model-tracked sun rays

Removed the Boreal palette and the aurora baking. Reworked the ambience into a dusty-library look split
into a baked static layer and a live light layer:

- **Static (baked, cached texture)** — `Material` enum (None/Paper/Wood/Stone/Marble) procedurally
  generated from value-noise/fBm: wavy vertical wood grain, mottled stone with thin cracks, turbulent
  marble veins, fibrous paper. Plus a soft top-down ambient light, an edge vignette, settled fog/dust
  (denser toward the floor), and a fine dust speckle. One `ColorImage` → `TextureHandle`, rebuilt only on
  resize/theme-switch (size coarsened to 32px, long side capped at 1536 — materials tolerate slight blur;
  raise for crisper grain at more rebuild cost).
- **Live (per-frame mesh)** — `paint_light` draws soft triangle shafts + radial glows as ONE mesh: a fixed
  window sunbeam (parallel shafts raking down-right through the dust, always on), a sunburst that tracks
  the cursor (`set_cursor_light` ← pointer each frame), and a warm hearth glow at the model's position
  (`set_model_light`). Only redraws when the pointer moves, so it's free at idle — no continuous repaint.

Per-palette materials: Reading Room=Paper, Archive=Wood, Slate=Stone, Folding@home=Marble, and a new
**Old Library** palette (deep walnut, candle-cream ink, brass) = Wood with the full warm sun + fog as the
showcase. Picker still lists five. "Model position" is pinned to the header model picker (captured via
`ui.scope(...).response.rect`); repointing it to the active node in the spatial view is a follow-up.

Validated: theme.rs type-checks clean against egui 0.34 in the scratch crate (forced 39s recompile, rc=0;
the `epaint::Mesh` / `colored_vertex` / `add_triangle` / `painter.add(mesh)` light path all compile). Lean
CLI + 58 tests unaffected (gui-gated). VISUAL + PERF TUNING HARDWARE-PENDING — material noise frequencies,
the layer alphas (ambient 0.10, vignette 0.20, fog 0.12, grain ≤0.30, shafts/glows 12–30/255), ray
counts/lengths, the window-beam angle, and the 1536 cap are first-guess values; if the resize/theme-switch
rebuild hitches on the per-pixel fBm, lower the cap or we add rayon.

---

## `:todo` — small-model TODO scan → "weak link" edges (merged context + KV-order)

New `:todo [ids] ['focus']` command (CLI, run-validated). It spins off the pool's **small** model with a
**small context** (each turn truncated to ~420 chars, one turn per call, no graph nodes pushed) in two
cheap passes:

1. **Extract** — for each turn in scope, ask the small model for one open item (a task/question/TODO) or
   `NONE`.
2. **Match** — for each open item, a cheap keyword pre-filter (local `kw()`: alnum words ≥4 chars, stop-
   words out) ranks the other turns by overlap, then the small model gets a YES/NO on only the top ~4
   candidates. A YES creates a **`WeakLink { from, to, label, weight }`** (label `addresses`, weight =
   keyword overlap decayed by candidate rank).

`WeakLink` is a new edge type **alongside** the parent/lineage tree — cross-branch labeled edges. They
power two things:
- **Merged context** — `:mergectx` now expands a `~label` ref token (e.g. `:mergectx ~addresses`, bare `~`
  = every label) to all nodes touched by weak links of that label, unioned with any normal refs. So the
  weakly-linked turns get merged into one context immediately, via the existing machinery.
- **KV-cache traversal order** — `kv_order(ids)` unions the weak-link connected components and emits each
  group contiguously (groups by earliest member). Prefilling/decoding related turns back-to-back keeps the
  engine's anchor pool (FIFO, capped) warm instead of evicting it between unrelated turns. `:todo` prints
  the suggested order; wiring it into the batched generate path is the next step.

Validated: 62 lib tests pass (added `parses_todo`; `kv_order_groups_weakly_linked_nodes`,
`link_refs_expand_label_tokens`, and `todo_links_open_items_via_small_model` — the last drives a YES-mock to
prove a link is actually created on overlapping turns). Live mock run: 4 open items extracted, KV-order
printed, no panic. `convograph` is gui-gated, so the runner uses its own local `kw()` rather than depending
on it. GUI arm is a CLI-only note (single-model there).

## Ambience v3 — animated fog frames + RTX-ish light (baked + blended, per your suggestion)

Made the static look move without per-frame per-pixel cost, the way you proposed (bake frames, loop with
random blending):
- **Surface vs fog split.** `build_surface_image` bakes the static layer (material + ambient light +
  vignette + dust speckle), cached. Fog is no longer baked into it.
- **Drifting fog.** `build_fog_image(variant)` bakes a small (~384px) low-frequency haze frame; FOG_FRAMES=4
  variants are baked once per palette/size. At runtime two of them are crossfaded with a **random schedule**
  (each segment retargets to a new frame over a random 2.5–6s via a tiny xorshift), smoothstepped so the two
  densities sum to ~1 (no flicker). That's a couple of image draws per frame; the bakes are cached.
- **RTX-ish light.** `paint_light` gained a `time` arg: two-layer **bloom** (wide halo + bright core) on
  every source, gentle **sway** on the window shafts, a slow **rotate + pulse** on the cursor sunburst, a
  pulse on the model hearth, and ~16 **sunlit dust motes** drifting on Lissajous paths that brighten as the
  cursor nears them.
- app.rs passes `ui.input(|i| i.time)` to both paint calls and `request_repaint_after(40ms)` while lit, so
  it animates at ~25fps. Pointer motion already repaints; this just keeps the fog/motes alive at idle.

Validated: theme.rs type-checks clean against egui 0.34 in the scratch crate (rc=0 — the fog cache, tinted
`painter.image`, `circle_filled`, `Pos2::distance`, the FOG_ANIM cell, and the time-driven light all
compile). VISUAL + PERF still HARDWARE-PENDING: the fog peak alpha (0.18), 4-frame count, 384px fog res, the
crossfade durations, bloom radii/alphas, mote count, and the 40ms repaint are all first-guess. The two
things to watch on the M2/M4: the per-pixel fBm bake on resize/theme-switch (lower the 1536 cap or add
rayon if it hitches), and the continuous repaint while lit (drop to ~20fps, or I can add a reduce-motion
toggle, if battery/idle CPU bothers you).

---

## Canopy — a living-tree theme: leaves driven by the graph, sun driven by the clock

New **Canopy** palette (sun-dappled forest — leaf-litter floor, warm bark, foliage overhead), a procedural
take on the Archive-with-canopy mockup. Two things make it "living":

- **Leaves take cues from the graph.** The chat tab exposes `canopy_cues() -> (graph_size, active_gens)`
  where `active_gens = stream-in-flight + queue depth + (fan running)`. app.rs feeds it via
  `theme::set_canopy(nodes, active)` each frame. In `paint_canopy`: the leaf count grows with the graph
  (70 → ~300 as turns accumulate, so the tree literally fills out as the conversation grows); the leaves
  sway in a wind loop whose amplitude + shimmer scale with `active_gens` (a busy graph = a rustling canopy);
  and a new turn (node count ticked up) fires a 0.7s brightness pulse that ripples through the foliage.
  Leaves are one mesh of small rotated quads (~300 max → cheap), backlit by the sun colour. A few dappled
  light spots flicker on the floor in time with the leaf motion.
- **Sun takes the system clock.** `sun_from_clock` reads `SystemTime::now()` and maps the hour of day to the
  sun: a low warm shaft angle at dawn/dusk (golden), steep + bright + near-neutral at noon, dim + cool
  (moonlight) at night. It drives the shaft direction, colour, and intensity for the canopy theme (other
  themes keep the fixed warm window beam). There's a `SUN_TZ_OFFSET_SECS` const (default 0 = UTC) — set it
  to your local UTC offset (e.g. `-5*3600` for EST, or for Montréal `-4*3600` in summer) so the on-screen
  noon lines up with your actual midday. Avoided pulling in `chrono`/`time` to keep the GUI deps stable; if
  you'd rather have automatic local time, that's the one dependency to add.

`Palette` gained a `canopy: bool` (the five existing palettes are `canopy: false`). The animation rides the
same `request_repaint_after(40ms)`-while-lit loop as the fog/motes.

Validated: theme.rs type-checks clean against egui 0.34 in the scratch crate (rc=0 — `sun_from_clock`'s
SystemTime, the leaf mesh, the canopy thread-locals, and the tints all compile). `canopy_cues()` reads only
existing Chat fields (`msgs`/`stream`/`queue`/`fan`). Lean CLI + 62 tests unaffected (all GUI-gated). VISUAL
HARDWARE-PENDING: leaf count/size, the upper-band height (0.42) and right-side cluster, sway/shimmer/pulse
magnitudes, the day-cycle colour ramp, and the dappled-floor spots are all first-guess; the procedural
foliage won't match the painted mockup's realism, and tuning it (or compositing an actual canopy image as
the surface) is the obvious next step once you can see it on the M2/M4.

---

## mesh3d — a software mesh renderer (past widgets, into game-engine territory)

New `mesh3d` module (GUI-gated): a tiny forward renderer that takes real 3D meshes, shades each vertex with
a directional light (Lambert + ambient), depth-sorts the triangles back-to-front (painter's algorithm), and
emits the whole thing as ONE `egui::epaint::Mesh`. It reuses `OrbitCam::project` for world→screen, so the
3D geometry lines up exactly with the spatial conversation graph (same camera, same scale).

- **Primitives:** `uv_sphere` (smooth), `tube` (cylinder wall, for edges/branches), `rock` (a noise-
  perturbed sphere rebuilt with per-FACE normals → faceted stone), `cairn` (a stack of shrinking rocks —
  the zen-stack motif from the Mistfall inspiration). All take an albedo colour.
- **Lighting:** `Light { dir, color, ambient }`; `sun_dir_3d()` derives a 3D sun direction from the real
  system clock (same day-cycle idea as the canopy's `sun_from_clock`, with the same `SUN_TZ_OFFSET_SECS`
  knob for local midday).
- **Renderer:** `render(meshes, cam, cx, cy, scale, light) -> Mesh` — per-vertex shade, drop triangles
  behind the camera, sort far→near, emit. No z-buffer (that's the next rung: a depth-tested wgpu paint
  callback with real shaders), but back-to-front is correct for the convex shapes here.
- **Wired in:** the spatial graph view now renders a lit 3D scene — `graph_scene` builds a shaded orb per
  node (coloured by conversation section, the active tip highlighted) and a tube per parent edge — drawn
  with the same camera, UNDER the existing dots/labels (which still handle hit-testing + text). So nodes
  read as lit orbs strung on tubes instead of flat dots.

Also added a **Mistfall** palette (cool fog-bound slate/teal, after the misty-grotto inspiration) — a serene
counterpart to the warm Canopy. Seven palettes now.

Validated: `mesh3d` compiles against egui 0.34 AND its 5 unit tests pass in the scratch crate (sphere
counts, shade floor/ceiling, render produces whole triangles, sun direction is unit, graph scene = orbs +
edges) — alongside spatial's own 12. theme.rs still clean; lean CLI + 62 tests unaffected (mesh3d is GUI-
gated). HARDWARE-PENDING as usual: orb/edge radii (0.26 / 0.05 world units), the ambient floor (0.32), and
whether to drop the old flat dots/lines once the orbs read well are all first-guess and easy to tune. The
honest ceiling: this is CPU transform + shade with painter's-algorithm sorting — great for orbs/rocks/
cairns, but for dense intersecting geometry or true per-pixel depth you'd want the wgpu paint-callback path,
which is the natural next step now that the mesh/scene/lighting plumbing exists.

---

## Scene-modeled graph: stones/leaves, rippling strands, glowing rune-IDs, carved tabs

Pushed the graph from "lit orbs" toward the inspiration scenes — each node is now a body chosen by palette,
strung on living connectors, with its identity inscribed as a glowing rune.

- **Per-scene node bodies** (`mesh3d::node_mesh` + `NodeStyle`): Mistfall/Slate → faceted **stone**, Canopy
  → **leaf-puff** (a cluster of leaf quads), everything else → **orb**. Coloured by conversation section,
  active tip in cyan, all shaded by the clock-driven sun.
- **Rippling strands** (`mesh3d::strand`): edges are no longer straight tubes but tubes around a wavy
  centreline that bows out mid-span (anchored at both nodes) and undulates with time — a vine / water
  trickle / gravity-air perturbation between the stones. Animated via a 40 ms repaint.
- **Glowing identicon runes** on the near stones: each node's short-id is rendered as a symmetric 5×5
  identicon (`mesh3d::rune_cells`, mirrored left↔right) drawn as emissive squares scaled to the stone's
  projected size and gently pulsing. The rune and the printed short-id share the same 64-bit hash
  (`vimmode::short_id_seed`), so they encode the SAME identity — `short_id` now derives from it.
- **Carved-stone tabs** (`theme::carved_tab`): the bottom switcher tabs get a raised bevel (lit top-left,
  shadowed bottom-right); the active tab adds a pulsing glowing engraving + a light sweep travelling across
  the inscription. Animated.
- New **Mistfall** palette (cool fog-bound slate/teal) from the prior step pairs with all of this.

Validated: `mesh3d` + `theme` compile against egui 0.34 and `mesh3d`'s unit tests pass in the scratch crate
(now 7: + `node_styles_and_strand_build`, `runes_are_symmetric_and_stable`), alongside spatial's 12 → 19.
Lean CLI + 62 tests unaffected; the `short_id` refactor keeps its stability test green. chat.rs/app.rs
wiring is GUI-gated → compile-pending on your hardware.

HARDWARE/REALISM-PENDING: this is still CPU mesh rendering through egui's painter — stones/leaves/strands
read as stylised low-poly, not the painted mockup. The runes, strand ripple, sun, and tab glow are all
first-guess magnitudes. The honest path to "near-replicate the images": (1) composite the actual scene
artwork as the baked background surface (the mockups are gorgeous as a static plate), (2) keep these meshes
+ the animated light as the live layer on top, and (3) for true depth/refraction/bloom, move the mesh pass
to a wgpu paint callback with real shaders — the geometry, scene assembly, lighting, and id-runes are now in
place to feed that pass.

---

## scene3d — baked 3D environments: textured models, live sun, floating

Built the actual scenes as 3D geometry (no image plates), lit by the clock-sun and floating. New `scene3d`
module + extended `mesh3d`:

- **Textured models (procedural vertex colour).** `mesh3d::Mesh3` gained `vcol` (per-vertex albedo) and
  `tint_up` / `tint_height` — moss on upward faces, wet-stone sides, bark gradients by height. No images;
  the "texture" is computed from each vertex's normal/height. New primitives `torus` (the moon-gate portal)
  and `disc` (ground/water/island tops).
- **Live sunshine.** `mesh3d::Light::sun(dir, colour)` is now a sun + a HEMISPHERICAL ambient (a cool sky
  fill from above, a dim warm bounce from below); `render` shades each vertex with sky/ground ambient +
  sun key. The sun direction is the same clock-driven `sun_dir_3d()`, so the whole scene tracks time of day.
- **Floating.** `Mesh3.offset` is added at render time, so an object bobs/drifts without rebuilding its
  geometry. `scene3d::Scene` carries a `Float { amp, speed, phase, drift }` per object; `animate(t)` just
  updates offsets each frame. The geometry is BAKED once per palette (thread-local cache) — only the floats
  move per frame.
- **Themed sets** (`render_env`, drawn behind the graph so the node-stones sit in front):
  - **Canopy** — mossy ground, a bark-gradient tree (trunk + branches + a foliage dome of leaf-clusters),
    and drifting leaves.
  - **Mistfall** — a dark water plane, a standing moon-gate portal ring, and floating stone islands (mossy
    tops, wet sides) topped with cairns, all bobbing.
  - everything else — floating stones over a plinth, so every theme gets depth.

Validated: `mesh3d` + `scene3d` compile against egui 0.34 and their unit tests pass in the scratch crate
(now 21 total: +`scenes_build_and_float`, +`float_still_is_zero`, and the shade test updated for the
hemispherical signature) alongside spatial's 12. Lean CLI + 62 tests untouched (all GUI-gated). chat.rs
wiring is compile-pending on your hardware.

Toward the end goal: this is the "baked scene + 3D models + live sun + floating" you asked for, all CPU →
egui painter. Triangle budget per frame is a few thousand (tree/portal/islands + graph), depth-sorted —
fine on M-series, but if it bites, the cache already means geometry is built once; only projection+sort run
per frame. The remaining gap to the mockups is per-pixel polish (soft shadows, god-ray volumetrics,
refraction through the portal/water, bloom) — that's the wgpu paint-callback pass, and `scene3d` is exactly
the scene description that pass would consume. Knobs (object counts, float amp/speed, tint colours, ambient
strength, the SUN_TZ_OFFSET_SECS for local midday) are all first-guess and easy to tune once you see it.

---

## Real-renderer framework: GPU GLSL pass (egui_glow), pixel-aligned with the projection

Committed to the GPU path. The eframe shell uses the **glow (OpenGL) backend** (not wgpu), so the right
framework is an `egui_glow` paint callback running GLSL — built as the actual end framework, not a demo,
split so the part I can prove correct is proven and the part I can't compile here is isolated + gated.

**`gpuscene` (the GPU-ready core — fully unit-tested, no GL types):**
- `Camera::from_orbit` builds a true 4×4 view-projection that **reproduces `OrbitCam::project` pixel-for-
  pixel** — verified by a test that compares the matrix's NDC→screen against `project()` for sample points
  (sub-pixel). This is the linchpin: it guarantees the GPU scene stays aligned with the 2D dots / runes /
  labels (which are placed by `project`). Also `eye()` (for fresnel/fog) and view-space `depth` (for sort).
- `pack_sorted(meshes, cam)` flattens the scene to an interleaved, back-to-front-sorted triangle soup
  (pos, normal, colour; mesh `offset` applied for floating) — painter's-algorithm order, so no depth buffer
  is needed for these convex shapes.
- `SHADER_VS` / `SHADER_FS` — the GLSL: vertex MVP; fragment hemispherical sun (matching `mesh3d::shade`) +
  a fresnel rim (wet stone / portal glow) + exponential distance fog. Version header (300 es vs 330) is
  prepended by the backend.

**`glow_scene` (the GL renderer — feature `gpu-scene`, HARDWARE-COMPILE-PENDING):** an `egui_glow::CallbackFn`
that compiles the program (via `egui_glow::ShaderVersion` for the right `#version`), uploads the packed
vertices, sets the uniforms, and draws — alpha-blended, depth-test off (CPU sort), into egui's pass at the
scene rect. Uses `eframe::glow` + `eframe::egui_glow` (re-exported by the glow backend) so it needs NO new
crates and exact-version-matches eframe. This is the only file that can't be compiled in the dev sandbox;
most-likely first-compile fixups (the `ShaderVersion`/`Painter::gl`/`CallbackFn` signatures) are flagged
inline.

**Wiring:** `spatial_ui` builds the graph meshes (scene-styled bodies + rippling strands) once, then a
`cfg` split — default builds keep the CPU `mesh3d::render` path (unchanged, validated); `--features
gpu-scene` packs env+graph through `gpuscene` and submits the `glow_scene` callback (env behind graph). The
2D dots/runes/labels still draw on top, aligned by the matching camera. New `scene3d::with_env` hands the
cached, animated environment meshes to the packer without a rebuild.

Validated: gpuscene + scene3d + mesh3d + theme compile and their unit tests pass in the scratch crate — now
**24** (incl. `view_proj_matches_orbitcam_project`, the alignment proof, and `pack_is_sorted_and_shaped`).
Lean CLI + 62 tests untouched; the `gpu-scene` deps were intentionally NOT added (eframe re-exports glow),
so dependency resolution for the normal build is unchanged. Default GUI build = the CPU path (well-trodden);
`glow_scene` only compiles under `gpu-scene` on your hardware.

To run it: `cargo run --features gpu-scene`. Set `gpuscene`'s camera near/far or `mesh3d::SUN_TZ_OFFSET_SECS`
as needed. The next rung within this framework — once it's compiling — is the per-pixel polish the mockups
have: an offscreen FBO with a real depth buffer (correct overlaps for non-convex geometry) + a bloom/
god-ray post pass; the GLSL + scene packing are already shaped to feed it.

---

## llvmpipe / Mesa: the GPU renderer is now VALIDATED (not "pending")

Took your suggestion and brought up Mesa's software GL (llvmpipe, Mesa 25.2.8) headless via a surfaceless
EGL context — no GPU, no display needed. That turned the one blind piece of the framework into a tested one,
on several independent fronts:

1. **The GUI compiles in-crate.** `cargo build --lib --features gpu-scene` builds the whole thing against
   real **eframe 0.34.3 / egui_glow / glow** — including `glow_scene.rs` and the spatial-view GPU wiring,
   which had never been compiled before. Default `--features gui` (the CPU path) also compiles. Only two
   warnings, both pre-existing egui-0.34 deprecations (`SidePanel::right`/`default_width`) unrelated to this
   work. So the GUI is no longer "compile-pending on hardware" — it compiles here now (deps are ~11 s
   incremental once cached).
2. **All 208 in-crate tests pass** under `--features gpu-scene` — the real crate, not the scratch proxy —
   including `view_proj_matches_orbitcam_project` (the camera-alignment proof) and the packing tests.
3. **Both shader paths compile on a real GL driver.** Extracted the exact `gpuscene::SHADER_VS/FS` source and
   compiled + linked it through llvmpipe: the `#version 300 es` path (web/ES) AND the desktop `#version 330`
   path — the latter is what your native M-series build uses (GL 4.5 core here). A draw to an offscreen FBO
   rendered the expected lit pixel `(227,255,181)`, which matches the hemispherical-sun + fresnel + fog math
   to the bit — so the fragment shader isn't just compiling, it's computing the right thing.
4. **The exact `glow` 0.16 call sequence runs.** A Rust harness mirroring `glow_scene::Renderer::{new,paint}`
   (same `glow` calls: program/shader build, VAO/VBO, attrib pointers, uniforms, `draw_arrays`, `read_pixels`)
   executed headless on llvmpipe and produced the identical pixel — confirming every `glow` method/enum used.
5. **The egui_glow integration is canonical.** The `egui::PaintCallback { rect, callback:
   Arc::new(egui_glow::CallbackFn::new(move |_info, painter| …)) }` + `painter.gl()` pattern matches egui's
   own `custom_3d_glow` example line-for-line. I dropped the earlier `ShaderVersion::version_declaration()/
   is_embedded()` calls (whose names I couldn't confirm) in favour of the example's `cfg!(target_arch)`
   version string — guaranteed-correct and one less version dependency.

Net: the design is taken all the way and the framework is real and checked — camera, packing, both shader
paths, the glow pipeline, and the egui_glow glue. The only thing not done is running the full *windowed* app
(that needs winit + a virtual display), but the library compiles and the GL pipeline demonstrably renders
under llvmpipe, so first run on your hardware should be about tuning (object counts, fog/ambient, float
amp/speed, `SUN_TZ_OFFSET_SECS`), not fixing. Run it with `cargo run --features gpu-scene`.

Dev-workflow note: the GUI now builds in-sandbox, so from here I can compile- and test-check the egui code
directly (incl. the glow path) rather than only via the scratch crate.

---

## Polish rung: offscreen FBO + depth buffer + bloom + god-rays (validated on llvmpipe)

The GPU path is now a multi-pass renderer instead of a single direct draw. Flow (all in `glow_scene`, fed by
`gpuscene` shaders):

1. **Scene → offscreen FBO with a real DEPTH buffer.** The 3D scene renders into an RGBA8 colour texture +
   a `DEPTH_COMPONENT16` renderbuffer, with `GL_DEPTH_TEST` on (opaque, cleared to transparent). This fixes
   overlaps for interpenetrating geometry that the CPU centroid-sort can't (e.g. a strand passing through a
   stone). Validated headless on llvmpipe: FBO reports COMPLETE and a near triangle correctly occludes a far
   one through the depth buffer.
2. **Bloom.** A bright-pass (`SHADER_BRIGHT_FS`, luma threshold) extracts highlights at half-res, then a
   separable Gaussian (`SHADER_BLUR_FS`, H then V) softens them — the glow on sunlit stone, the portal, the
   cyan active node.
3. **God-rays.** `SHADER_GODRAY_FS` marches 24 steps from each pixel toward the sun's *screen* position,
   accumulating the bright texture — the volumetric shafts the mockups have. The sun's screen uv comes from
   the new `gpuscene::Camera::dir_uv` (projects the sun direction's vanishing point; returns `None` when the
   sun is behind the camera, in which case god-rays fade out).
4. **Composite** into egui's framebuffer (`painter.intermediate_fbo()`): scene + additive bloom + god-rays,
   blended over the egui content so glow halos bleed past geometry edges. Verified to be a faithful
   orientation-preserving passthrough (no vertical flip), so combined with the proven camera matrix the
   result stays pixel-aligned with the 2D dots/runes/labels.

The targets (scene colour+depth, two half-res ping-pong buffers) live in `glow_scene` and resize with the
panel; per frame only the float offsets + uploads change. State is restored carefully (egui's intermediate
FBO + viewport are saved and rebound before the composite), per egui_glow's own guidance.

Validation, all green: every shader (scene + bright/blur/godray/composite) compiles on Mesa/llvmpipe in BOTH
`#version 300 es` and `#version 330`; the whole GUI compiles in-crate with `--features gpu-scene` against
eframe 0.34/egui_glow/glow (so every new `glow` call — renderbuffers, depth attachment, samplers, multiple
texture units — is real-API-checked); the offscreen+depth+bloom+composite pipeline was run headless on
llvmpipe (FBO complete, depth occlusion correct, orientation correct); and all **208 in-crate tests** pass.
The god-ray pass is shader- and compile-validated; its only un-pixel-tested bit is the radial march itself,
which is a standard accumulation.

Knobs to tune on hardware (all in `glow_scene::Uniforms`, currently first-guess): `threshold` 0.62,
`bloom_amt` 0.85, `godray_amt` 0.45, `godray_weight` 0.42, `godray_decay` 0.92, blur radius 1.5 px, half-res
bloom. If bloom feels weak because the LDR scene clamps at 1.0, the next refinement is an `RGBA16F` scene
target (HDR) so highlights exceed 1.0 before the bright-pass — a one-line texture-format change here.
Run it with `cargo run --features gpu-scene`.

---

## Texturing: procedural materials + triplanar mapping (no UVs, no asset files)

The scene now has real textures, mapped onto blobby geometry without a UV unwrap. How it works:

**Create — `texgen.rs` (CPU, no asset files).** Bakes seamless, TILING material tiles from the same
value-noise/fBm family as `theme.rs`, but on a lattice that wraps at the tile period so edges match (no
seams when repeated in world space). Five materials: paper, wood/bark (anisotropic vertical streaks +
rings), stone (mottled grey + grain), water (low-contrast teal weave), moss/foliage (grainy green +
speckle). They bake once into one RGBA8 buffer (`bake_layers`) uploaded as a mipmapped, repeating
`TEXTURE_2D_ARRAY` (one layer per material). Unit-tested: every layer is deterministic, opaque, and
seamless (edge discontinuity < 60/255).

**Map — triplanar in the scene fragment shader.** No mesh UVs. Each material is sampled three times — along
world X/Y/Z planes — and blended by the normal (`pow(abs(n),4)`), so rocks, the trunk, and foliage all
texture cleanly with zero unwrap, and nothing stretches when meshes float/deform. World-space sampling +
`REPEAT` wrap gives consistent tiling; `u_texscale` sets the world-units-per-tile frequency. Water (layer 3)
scrolls its sample coords by `u_time`.

**Material selection.** `Mesh3` gained a per-vertex `mat` id (a texture-array layer), packed as a 4th
vertex attribute (vertices are now 11 floats: pos3, normal3, colour4, mat1). The existing per-vertex tint
(`tint_up`/`tint_height`) now MODULATES the sampled texture, so the palette still drives colour and the
texture adds grain/relief. The scene builders assign materials (canopy: bark trunk/branches, moss ground +
foliage; mistfall: water plane, stone ring/islands/cairns), and the graph picks per theme (strands =
vine/water/stone, node bodies = foliage/stone).

Validation, all green:
- `texgen` tiling + determinism unit tests pass; **210 in-crate tests** total pass.
- All shaders (now incl. the triplanar scene FS with `sampler2DArray`) compile on llvmpipe in both
  `#version 300 es` and `#version 330`. GL ES needs an explicit `precision highp sampler2DArray;` (added,
  version-aware, in `glow_scene::link`).
- The whole GUI compiles in-crate with `--features gpu-scene` — so every new `glow` call (`tex_image_3d`,
  `generate_mipmap`, `sampler2DArray` binding, the material attribute) is real-API-checked.
- Headless on llvmpipe: a `TEXTURE_2D_ARRAY` was uploaded and sampled triplanar through the REAL scene
  shader — layer selection works (the green layer was picked over the red) and the texture detail comes
  through (full checker variance). Run it with `cargo run --features gpu-scene`.

Knobs: `Uniforms.texscale` (0.35 default — bigger = finer tiling), per-mesh `mat`, the per-material noise
params + palettes in `texgen.rs`, `TEX_SIZE` (128). Natural follow-ons within this system: a second array
for **normal/roughness** maps (triplanar normal mapping for real surface relief), and an HDR (`RGBA16F`)
scene target so bloom on the textured highlights has more punch.

---

## Normal mapping + specular: real surface relief (validated on llvmpipe)

The flat albedo textures now carry RELIEF. Added a second procedural texture array (normal + roughness) and
triplanar normal mapping, so bark grooves, stone pits, and water ripple actually catch the light, with a
sun glint on the smooth materials.

**Create — normal maps from the height field.** `texgen` now factors each material into a `height(u,v)`
scalar field; the albedo is coloured from it (as before) and a NORMAL map is derived from its gradient
(central differences → tangent-space normal, encoded `n*0.5+0.5`), with per-material relief strength
(`bump`) and a base `roughness` packed into alpha (water/wet stone low = shiny; bark/moss high = matte).
`bake_normals()` produces a second `LAYERS`-deep array; both arrays upload through one generalized
`upload_array`. Unit-tested: every normal layer faces up (blue channel dominant) and tiles seamlessly.

**Map — triplanar normal perturbation.** The scene shader samples the normal map on all three world planes,
turns each plane's tangent slope into a world-axis perturbation, blends by the same triplanar weights, and
tilts the lighting normal (`N = normalize(n + pert·u_bump)`). No tangent frames, no UVs — robust on the
blobby geometry. Roughness drives a Blinn-Phong specular glint (sharper + stronger as roughness drops), so
water and wet stone shine while bark and moss stay matte. The albedo + normal stay registered (same world
coords + scale).

Validation, all green:
- `texgen` normal-map test (faces-up + tiling) passes; graphics + core tests pass (25/25 for
  texgen/gpuscene/mesh3d/spatial). (One unrelated `sandbox` offline-deps test flaked under low disk +
  parallel contention — it shells out to `rustc`; it passes run on its own. Not touched by this work.)
- The normal-mapped scene FS compiles on llvmpipe in both `#version 300 es` and `#version 330`.
- The whole GUI compiles in-crate with `--features gpu-scene` (two `TEXTURE_2D_ARRAY`s, the extra sampler +
  bump uniform all real-API-checked).
- Decisive headless test on llvmpipe: a flat grey quad lit from the side with a ±X-tilt stripe normal map
  shows luma range 0 at `bump=0` (uniform) and 227 at `bump=2` (strongly striped) — the normal map
  demonstrably perturbs the lighting per-fragment, through the real scene shader. Specular compiles + runs.

Knobs: `Uniforms.bump` (0.7), per-material `bump`/`roughness` in `texgen.rs`, `texscale`. The albedo and
normal arrays share layout, so adding more materials is one `height` arm + a colour pair + a bump/roughness
entry. Remaining graphics polish from here: the HDR (`RGBA16F`) scene target so bloom on the lit specular
highlights blooms harder, and (further out) shadow mapping for the sun. Run it with
`cargo run --features gpu-scene`.

---

## Themed scenes + brighter lighting (from on-hardware feedback)

Acting on the spatial-view screenshot. (Note: the 3D scene only renders in **Spatial** view, not Linear —
Linear is the flat text log by design; toggle ◫ Spatial, and use `--features gpu-scene` for textures/normals/
bloom.)

**Lighting was too dark — brightened both paths.** Hemispherical ambient up (0.55 → 0.72) with a brighter
ground bounce, shading gain 1.45 → 1.65, and a new `u_exposure` knob (default 1.35) on the GPU path — net
~2× brighter floor, tunable. (The clock-sun only steers direction for the scene, not intensity, so this
isn't a time-of-day artifact.)

**Per-theme environments** (`scene3d` now dispatches four, others fall back to floating stones):
- **Mistfall** — added floating water droplets: the surface breaks into drifting wet globules (WATER
  material → glossy), bobbing on big amplitudes because "gravity is strange in this room."
- **Archive** — NEW: warm wood floor + a rising column of autumn leaves caught in a magic gust (spiral-up
  placement, strong flutter/drift).
- **Reading Room** — NEW: pale paper floor + flying folded-paper origami drifting on currents (new
  `mesh3d::paper_fold` primitive — a faceted tent so each fold catches light, PAPER material).

Validated: the brighter scene shader compiles on llvmpipe (both `300 es` and `330`); the whole GUI compiles
in-crate with `--features gpu-scene`; `scene3d` build/float tests now cover Archive + Reading Room and pass
(9/9 scene+mesh tests). Tunables: `Uniforms.exposure`, droplet/leaf/paper counts + `Float` amp/speed/drift
per scene, the autumn/paper tints.

### Still open (from the same feedback)
- **Folding@home** — neuron/protein-folding scene: awaiting the updated reference-images package before
  building it (so it matches the inspiration).
- **Water "break up more"** — first pass is 16 droplets; easy to push count/size or fragment the plane itself.
- **Forefront UI as lit meshes** — the buttons/panels reacting to light + moisture. Honest take: truly
  rebuilding egui chrome as 3D geometry (aligned + clickable) is a large lift; the tractable path that gets
  the *look* is to extend the existing `carved_tab` treatment (procedural material + bevel + a light/moisture
  sheen that tracks the sun) to a `carved_button`/panel style. Proposed as the next UI rung.

---

## Carved-material forefront chrome (buttons + pills)

Extending the `carved_tab` look to the interactive chrome so buttons feel carved from the same material as
the room and "play with light and moisture."

New shared painter `theme::paint_carved` (and a `Carved` options struct) draws any rect as a carved-material
surface: a **translucent inset** (so the background grain reads through, a polished cut of the same
stone/wood/paper), a **sparse scatter of the palette's own `material_grain`** (so it's textured even over flat
areas or the 3D scene), a **directional bevel** (lit top-left / shadowed bottom-right, the carved_tab idiom)
with a lit cap + shadowed foot for relief, and the key new bit — a **specular "wet" sheen**: a soft bright
spot positioned toward the sun (via `sun_from_clock`, so it shifts with time of day), gently shimmering on
`time`, with a thin drifting glint that intensifies on hover. Hover/press brighten the fill, sheen, and an
accent glow ring.

Routed through it (so every call site upgrades at once):
- `theme::action_button` — the **Send** button and other actions (kept signature + press-shrink + ink lerp).
- `theme::pill` — clickable pills (filled = accent-tinted + stronger sheen; unfilled = muted).
- New `theme::carved_button(ui, text)` / `carved_button_tinted(ui, text, accent)` general helpers.

Wired in `tabs/chat.rs`: the **suggestion starter chips** ("Explain this code", etc.) → `carved_button_tinted`
in CYAN; **📎 Attach** and **⊹ Overview** → `carved_button`. (Quiet status tags via `chip`, and full panel
*frames*, are the natural next pass — left as-is for now.)

Validated: compiles in-crate under both `--features gpu-scene` and `--features gui`; full suite **211/211**
passing; only the two pre-existing egui-0.34 deprecation warnings remain (unrelated, chat.rs:1980). Visual
result is hardware-pending as usual. Tunables: per-call `sheen` scale, the bevel/grain alphas, sheen radius +
shimmer speed, and which call sites adopt the helpers.

---

## Spatial view: flowing river layout + gentle camera drift (from camera screenshots)

The screenshots showed the graph coiled into a tight radial knot with labels overlapping, and the camera too
zoomed / locked orbiting the ground plane. Two root causes, two fixes.

**Layout was a descending helix** (`spatial::layout`) relaxed by a repulsion `fold` — every node circled one
vertical axis, so it read as a radial ball. Replaced (for the spatial view) with a new **`spatial::flow`**: the
thread advances along +X by a *time-stretched* step (a long pause reaches further), gently meandering in Y and
Z at two frequencies so it snakes through the room, while each fork sends its children into **separate lanes
that flow alongside** — and descendants **inherit the lane**, so every sub-branch keeps its own channel. Same
relations as before (depth → distance along the flow, branches → lanes; staggered roots for independent
threads), but spread across the scene instead of coiled. Deterministic and O(n) (the old fold was O(n²·iters)).
`fold`/`layout` are kept (still unit-tested) but the view now calls `flow`. Because the camera auto-frames the
centroid and fits the bounding radius, the wider layout also **pulls the camera back on its own** — directly
addressing "too zoomed in."

**Camera did a full orbit fixed on the ground.** Added `OrbitCam::drifted(t)` — a render-time copy with a small
parallax sway in yaw/pitch (≈ ±4°, two rates) and a faint dolly, **additive and non-accumulating** so manual
orbit/zoom stay in control and it never runs off into a full revolution. `spatial_ui` now projects the dots,
hit-tests, and renders the 3D meshes (CPU + GPU paths) all through this one drifting camera, so everything stays
pixel-aligned while the scene gently breathes. Default view angle nudged to a higher 3/4 look; the orbit target
follows the flow's centroid (mid-air), not the ground plane.

Validated: compiles in-crate under `--features gpu-scene`; suite **215/215** (4 new: flow advances/spreads,
siblings take separate lanes, determinism/degenerate-safe, drift small + non-mutating). Visual is
hardware-pending. Tunables: flow `SEP`/step scale + meander amp/frequency, and the drift amplitudes/rates.

### Scene pack received (for next)
The uploaded `ui-inspirations` pack includes a 7-scene reference set — **1-foldingathome** (the protein/neuron
refs that were pending!), 2-grove, 3-misty, 4-marble, 5-beach, 6-observatory, 7-atlantis libraries — stashed for
building out per-theme environments (à la Canopy/Mistfall). The Folding@home scene is now unblocked.

---

## Scene feedback pass 1: roaming light, theme accents, higher-res meshes, Folding@home

Acting on the long scene-by-scene critique. This pass took the cross-cutting + clearly-scoped items; the big
build-outs are queued below.

**The cursor-attached "shiny thing" is gone.** `paint_light` no longer tracks the pointer — the global
sun-catcher now *roams on its own* on a slow Lissajous drift (softer + wider than the old cursor sunburst),
and the dust motes brighten where it passes. Paired with the spatial view's parallax camera, illumination now
plays across the scene by itself (the "grandeur" effect) instead of sticking to the cursor.

**Blue highlights in warm rooms fixed.** New `theme::accent()` returns a scene-appropriate accent — amber for
the wood/paper/marble libraries, teal for the stone/fog scenes, green for Folding@home — and the suggestion
chips use it instead of a hardcoded cyan. So Reading Room / Canopy no longer show a cold blue glow.

**Higher mesh resolution** (the "too polygonal / triangles not high-res" notes): node orbs 10×7 → 22×14
spheres; Canopy trunk/branches and every ground/water disc and the Mistfall portal ring and the water bubbles
all tessellated up (≈1.5–2×); and `mesh3d::paper_fold` rebuilt from a 4-triangle tent into a 5×5 creased grid
(ridge + cross-fold, 32 triangles, smooth-averaged normals) so origami reads as folded sheets.

**Folding@home scene built** (its reference folder arrived empty, so this is from the verbal brief): stones
removed; a tall DNA/protein **double-helix** centerpiece — twin backbone tubes, base-pair rungs, glossy
amino-acid-coloured residues — with molecules drifting through a dim cell membrane. Residues + conversation
nodes ride the PAPER material (palest) so `albedo = v_color·texture` keeps their colours true. **Canopy palette**
also warmed from green toward brown/wood per the reference.

Validated: compiles under `--features gpu-scene` and `--features gui`; suite **215/215**; only the two
pre-existing egui deprecations remain. Visual is hardware-pending.

### Queued from the same critique (bigger build-outs / need input)
- **Text on leaves** — engraving / sunlight-dappled info text on flat leaf faces (a real text-on-mesh feature).
- **Environment build-outs** — Canopy: triplet branching (trunk→3→3·3), leafy vines, a back wall of ancient
  books, dirt + full wood textures, wooden buttons with leather-trim corners + indigo dye. Reading Room: an
  actual environment + sunbeams + front-panel texture (papers-only today). Mistfall: rolling mist, stone/glass
  front textures, **reflective** water; and push every centerpiece toward photoreal (it's only roughed in).
- **Scene renaming** — the two grotto-style scenes are distinct ("misty low-fog waterfall" vs "ancient-alien
  lost-program"); send the names you want and I'll split/rename them.

---

## Enclosure geometry — the graph now lives inside a room

The concept-art brief's core insight: the references are dominated by *enclosure geometry and a dominant
centerpiece*, not by the UI widgets — the graph should live inside something, ~80% atmosphere / ~20% object.
This pass builds the enclosure that was missing (scenes were a centerpiece floating over a disc in a void).

New `mesh3d::wall_ring` — an inward-facing cylinder shell (normals point in, so the interior catches light
while the far side fades into fog). New `scene3d::enclose` wraps every built scene with: a wide sunken outer
floor (a raised central platform now reads as a platform, not the whole world), a tall circular **wall**
themed to the room — bookshelf **wood** for Canopy/Archive/Old Library, pale **limestone** for the Reading
Room, raw **stone** for Mistfall/Folding/Slate — banded with horizontal **shelving** (libraries) or **rock
strata** (grottoes), lit toward the top via height-tint, and an **oculus ring** high overhead hinting at a
dome (the "light from beyond the room"). Proportioned after the brief: room Ø ≈ 80 units, wall height ≈ ⅓ of
that, centerpiece ≈ 15% of the volume. Per-theme **fog** thickens the far wall into atmosphere (Mistfall rolls
heavy at 0.030; others ~0.017) — so architecture *emerges from fog* exactly as the brief describes. The
auto-fit camera keeps the viewer inside the wall (R=40 vs typical fit-distance ~29).

Validated: compiles under `--features gpu-scene` + `--features gui`; suite **216/216** (new `wall_ring_faces_inward`
checks vertices sit on the radius with inward normals; `scenes_build_and_float` now builds every *enclosed*
scene). Hardware-pending visually. Tunables: enclosure `R`/`H`, band count, per-theme wall colours + fog.

### How the brief maps to the build (roadmap)
Already in place: drifting light + parallax playing with GI; carved-material buttons; floating water droplets;
exp fog + god-ray pass; depth-sorted lit scenes. **Now:** enclosure walls + shelving + oculus + atmosphere.
**Queued, in the brief's own terms:** concentric stone **terraces** (branches) stepping platform→wall; a real
translucent **dome** overhead with moving patterns ("global context / latent space, visible but unreachable");
**memory beneath reflective water** (older/forgotten branches seen downward through the surface); **stone
buttons** with text **carved** into rock or glowing as **mineral veins**; **gravitational air ripples** around
significant nodes (heat-haze/lensing distortion); **droplet lenses** (tiny reflected text inside drifting
droplets); **reflective stone floors**; **leaves as displays** (sunlight through translucent leaves reveals
text that dances as branches sway) + **light-projected text** onto stone; **presence hints** (an open book, a
page turning, ripples) implying the archive was just inhabited. Plus pending earlier: triplet tree branching,
Mistfall reflective water, scene renaming.

---

## Completing the architecture (terraces + oculus dome) + chrome carved from rock

Two more pieces of the concept brief, building on the enclosure.

**Architecture closed off.** Two new primitives — `mesh3d::annulus` (a flat terrace ring) and `mesh3d::dome_cap`
(an inward-facing dome that narrows to an oculus, normals facing in/down so the underside lights). `enclose`
now adds **three concentric terraces** stepping out from the central platform toward the wall (the brief's
nested rings = branches) and a **dome** overhead narrowing to an **oculus skylight** — so each room is now a
full vessel (platform → terraces → banded wall → dome → oculus), light implied from beyond. Quarter-sine dome
curve; per-theme wall colour/material carry through.

**Chrome grows out of the rock.** New `theme::carved_label` renders button text as if **chiselled into stone**
— a dark shadow pressed in below-right and a lit lip above-left give engraved relief — with an optional
**mineral-vein glow** bleeding from the inscription (laid out per-colour so each pass tints correctly). Wired
through `action_button` (the primary action glows like a warm vein, steady and brighter on hover) and
`carved_button`/`carved_button_tinted` (engraved always; accented buttons light up as glowing veins on hover).
So the labels read as inscriptions in the carved-material buttons rather than flat text on top.

Validated: compiles under `--features gpu-scene` + `--features gui`; suite **217/217** (new
`annulus_and_dome_are_well_formed` checks terrace up-normals + the dome narrowing/rising to its oculus;
`scenes_build_and_float` builds every room now with terraces + dome). Hardware-pending visually. Tunables:
terrace count/heights, dome rise + oculus radius, and the engraving offsets + glow strength per button state.

Still queued from the brief: **reflective water memory** (the hardest — needs a reflection pass or translucent
water with mirrored geometry, so older branches read downward through the surface); a **live dome** with moving
latent-space patterns; **gravitational air ripples** lensing around significant nodes; **droplet lenses**;
genuinely **reflective stone floors**; **leaves as displays** with light-revealed dancing text; **presence
hints**; plus earlier pending: triplet tree branching, scene renaming.

---

## Reflective-water memory + gravitational air ripples ("do one then the next")

Two more beats from the brief, both in the spatial view's render.

**Reflection — the surface remembers the conversation.** Each node and strand is now mirrored onto the
reflective plane (water for Mistfall, polished limestone for the Reading Room, damp wood/dirt for
Canopy/Archive — per-theme strength + plane height; Folding has none). Each ghost is faded toward the
background, **dimmer where the real node rides higher** (a glancing reflection) and **dimmer with age** (older
turns soften into the surface), so the past reads as memory held in the water. Implemented as blended ghosts
on the surface, appended to the graph mesh pass so they tint the opaque water/floor via alpha blending — true
sub-surface refraction would need a translucent-water pass and is noted as a follow-up.

**Gravitational air ripples — significance you feel, not a label.** A token-heavy node now visibly *bends space
around it*: faint concentric rings breathe outward at staggered phases and a few motes orbit it on a
perspective-squashed path, all in the theme accent and fading with distance. Threshold-gated on token "heat"
so only the weighty turns get a field; the brief's "the viewer subconsciously understands this thing is
significant" without an icon.

Validated: compiles under `--features gpu-scene` + `--features gui`; suite **217/217** (both are render-pass
additions over already-tested primitives/layout). Hardware-pending visually. Tunables: per-theme reflection
strength + plane height, the height/age fade curves; ripple significance threshold, ring count/phase, mote
orbit + the squash factor.

Still queued: true sub-surface (translucent) water; a live dome with moving latent-space patterns; **droplet
lenses** (tiny reflected text inside drifting droplets); **leaves as displays** with light-revealed dancing
text; **presence hints** (an open book, a page turning, a stray ripple); plus earlier pending — triplet tree
branching, scene renaming.

---

## Variant selector — compare 10 candidate looks live (no recompile per idea)

New header dropdown **"variant"** beside the theme picker (1–10), so each round of design work can ship ten
candidates at once and you pick favourites in the running app — turning the slow edit→compile→exchange loop
into an instant in-app A/B. `theme::variant()` / `set_variant()` hold the choice (thread-local, clamped 1–10);
`theme::variant_label()` names each so it's always clear what's being compared.

**This round the variant sweeps the scene atmosphere / colour-grade** (`theme::grade(v)` → exposure, fog ×,
bloom amount + threshold, ambient ×, sun warmth), applied to the GPU scene uniforms in `spatial_ui`:

  1 baseline · 2 brighter · 3 airy/bright · 4 moody-dramatic · 5 warm golden-hour · 6 cool moonlit ·
  7 dreamy heavy-bloom · 8 crisp low-bloom · 9 heavy atmosphere/fog · 10 punchy high-contrast

(Each is a distinct exposure/fog/bloom/ambient/warmth combination; they affect the GPU path — `--features
gpu-scene`.) Switch in the header and the scene re-grades instantly across whichever theme you're in.

Going forward, each round I'll redefine what 1–10 map to and list them here + in `variant_label`, so candidates
can be compared without a rebuild. Validated: compiles under both feature sets; suite **218/218** (new
`variant_clamps_and_grades_are_distinct` checks the clamp + that the ten grades are meaningfully different).

---

## Variant round 2 — surface + geometry DETAIL (texture resolution + object construction)

Repointed the variant dropdown (1–10) at **detail**: how finely textures are baked, how densely they tile,
the normal-relief strength, and how finely the scene objects are tessellated — so the jump in texture
resolution and the geometry built around it can be compared live. The atmosphere/grade is **locked**
(`theme::LOCKED_GRADE = 2`, the brighter look) so only detail changes between candidates.

`theme::detail(v)` → (texture px, texscale, bump, tessellation ×):

  1 128px×1.0 · 2 128px×1.2 · 3 192px×1.4 · 4 192px×1.6 · 5 256px×1.8 · 6 256px×2.0 ·
  7 320px×2.3 · 8 384px×2.6 · 9 448px×3.0 · 10 512px×3.4 (finest + densest, heaviest)

Plumbing so it all updates live on a dropdown change:
- **Texture resolution** — `texgen::bake_layers/bake_normals` now take a size; the GPU `Renderer` tracks the
  baked resolution and **re-bakes + re-uploads** both material/normal arrays when the tier changes (same
  pattern, finer texels + sharper normal gradients). Higher tiers genuinely raise texel density.
- **Texture tiling + relief** — `texscale` and `bump` uniforms driven by the tier (finer tiling, stronger
  normal relief).
- **Object construction** — new global `mesh3d::set_tess_mul`; every procedural primitive (sphere, tube,
  disc, torus, wall, terrace, dome…) scales its segment counts by it, and the scene cache is now keyed by
  `(theme, variant)` so the whole environment re-tessellates when the tier changes. Graph node bodies scale
  too (rebuilt per frame).

Validated: compiles under `--features gpu-scene` + `--features gui`; suite **219/219** (new: textures bake at
64/128/256/512px with correct sizes and an identical pattern at finer sampling; the ten detail tiers are
distinct with non-decreasing resolution + strictly rising tessellation). The live GL re-upload mirrors the
already-proven array-upload path (runtime-pending on hardware). Switching the dropdown re-bakes + re-builds
instantly; at tier 10 the wall is ~330 segments and textures 512px, so it's the cost ceiling to feel out.

To lock a winner: tell me the tier you like and I'll bake those numbers in as the baseline (and we keep the
dropdown for the next axis). Earlier round's grade is parked at `LOCKED_GRADE = 2`; say the word to relock it.

---

## UI "sandwich" — stable style API so toolkit/style iteration costs fewer tokens

Abstracted the chrome into five layers so that restyling (the thing we do most) touches one place instead of
rippling through call sites:

    L0  egui / eframe          raw immediate-mode toolkit
    L1  egui-pattern helpers   ui::row / row_rtl / combo — the repetitive egui idioms, written once
    L2  widget rendering       theme::paint_carved / carved_label — the carved-material look (the HOW)
    L3  app widget surface     ui::button / button_accent / primary / pill / chip / panel (what the app calls)
    L4  style tokens           theme::Skin — WHAT the look is, in ONE swappable struct (the stable API)

The key piece is **`theme::Skin`** (layer 4): every carved-material magic number that used to be buried in the
paint code — corner radius, fill opacity (rest/hover/down/disabled), accent tint, grain strength, bevel
lit/shadow, sheen amount + speed, mineral-vein glow, engrave shadow/lip — now lives as a named field. `Skin::carved()`
is the shipped look. `paint_carved`/`carved_label` read `skin()`; they no longer hold any constants.

Why this saves tokens going forward: a restyle is now "edit a few `Skin` fields" (e.g. `sheen: 0.0` for a matte
look, `radius: 2` for sharp corners) — not a hunt through painting code, and call sites never move. The L3
names (`ui::button`, …) are the contract; they don't change when the look does, so the API doesn't rot. `Skin`
is also swappable at runtime via `set_skin`, so a future variant round can A/B whole styles live.

`ui.rs` is a new module that documents the stack and exposes L1+L3+L4 under one namespace. The header theme +
variant dropdowns now go through `ui::combo`, proving the L3 surface end-to-end. Existing
`theme::carved_button(…)` call sites keep working and can adopt `ui::button(…)` whenever touched — migration is
incremental, not a flag day.

Validated: builds under `--features gpu-scene` + `--features gui`; refactor is value-preserving (Skin defaults
equal the old constants, so the look is byte-for-byte unchanged); suite **221/221** (new: carved Skin tokens are
sane; the Skin swaps and restores). Only the 2 pre-existing egui-0.34 deprecation warnings remain.

## Headless self-reflection harness (env-gated boot hook) — 2026-06-16
Goal: render the real egui app offscreen so visual work (themes/spatial/Atlantis) can be screenshotted
and iterated without a model, GPU engine, or user input. The renderer was ALREADY decoupled from the
engine (gpu-gemm is optional + workspace-excluded; `gui`+`gpu-scene` build with no engine), so this only
adds a launch hook, all inert unless `LOGSCOPE_BOOT` is set:
- chat.rs `Chat::boot_demo(spatial)` — seeds a small BRANCHING demo tree (reuses load_imported + a few
  sibling tips) and flips spatial_view/spatial_auto. Gives the spatial layout real nodes to frame.
- app.rs `LogscopeApp::boot_env()` (called at end of Default) — reads LOGSCOPE_TAB / LOGSCOPE_SPATIAL /
  LOGSCOPE_SEED, dismisses the first-run wizard.
- main.rs — LOGSCOPE_THEME picks a palette from theme::PALETTES by name (else default install).
Run headless: Xvfb :99 + LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe, then
  LOGSCOPE_BOOT=1 LOGSCOPE_THEME=Atlantis LOGSCOPE_TAB=chat LOGSCOPE_SPATIAL=1 ./logscope-egui
  import -display :99 -window root shot.png
Toolchain note: Rust 1.96 (offline bundle), Mesa llvmpipe (GL 4.5) + lavapipe (Vulkan 1.4 CPU) both
verified for the glow and wgpu paths respectively. Build proven: `cargo build --features gpu-scene` with
engine OFF (only 2 egui-0.34 deprecation warnings).
OBSERVATION for the design track: the Atlantis *palette* exists and tints the UI, but the bible's scene
richness (dome ribs, caustics, god-ray shafts) is NOT yet in the gpu-scene path — that port is the next
visual deliverable.

## scene-view fork — renderer-only page, populated from a .logscope template — 2026-06-16
A stripped binary for headless scene iteration: NO tabs/header/composer, the GPU spatial renderer ON by
default, full-window, populated from a conversation template (no model needed).
- chat.rs: `Chat::scene_ui(ui)` (forces spatial_view + renders only spatial_ui) and
  `Chat::load_logscope_script(text)` — parses the uploaded `.logscope` format (`>` = user turn,
  `:goto N` forks the next turn off main-line turn N, `:Nca`/`:Nfa`/`:fa` widen with sibling tips;
  assistant turns are synthesized stubs so nodes read as a real tree).
- scene_preview.rs: `ScenePreview` eframe::App (this eframe fork uses `App::ui(&mut Ui,&mut Frame)` +
  `show_inside`, NOT `update`). Template source: env `LOGSCOPE_CONVO`, else
  `examples/ladder/99-fullspine-marathon.logscope`, else the built-in demo.
- bin `scene-view` (required-features `gpu-scene`); theme via env `LOGSCOPE_THEME` (default Atlantis).
Run headless (from suite root so the default template resolves):
  Xvfb :99 ...; LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe LOGSCOPE_THEME=Atlantis \
    LOGSCOPE_CONVO=examples/ladder/99-fullspine-marathon.logscope ./scene-view ; import -window root shot.png
FIRST RENDER (Atlantis): the env already has a dome/drum + correct teal water; MISSING vs the reference =
caustics, god-ray shafts through the dome, warm lantern pools, fish, bookshelf enclosure; the node strand
also clips the dome (framing). That's the scene3d.rs Atlantis punch list.

## Fast-iteration linking + render harness — 2026-06-16
The edit→build→screenshot loop was link-bound (a ~330MB static debug binary re-linked every change).
Fixed via .cargo/config.toml: mold linker + `-C prefer-dynamic` (libstd as a .so) + dev profile
`split-debuginfo="unpacked"` + `debug=1`. Result: binary 330MB→107MB, incremental rebuild ~5min→~5s.
GOTCHA: prefer-dynamic binaries need `LD_LIBRARY_PATH=/usr/local/lib/rustlib/x86_64-unknown-linux-gnu/lib`
at runtime. `render-scene.sh <Theme> <out.png>` wraps build + Xvfb/llvmpipe + screenshot with that set.

## Pearl / fiber-optic graph rendering (Atlantis) — 2026-06-16
Leading with rendering (per the agreed order; :foreach parked in TODO for a later batch).
- mesh3d.rs: new `NodeStyle::Pearl` — a smooth high-tess uv_sphere (glossy → prismarine sheen, catches caustics).
- chat.rs spatial_ui: Atlantis routes nodes→Pearl, node+strand material→WATER (low-rough/glassy). Strands get
  `emissive = 0.55` (self-lit light-cords running parent→child, picked up by the bloom pass); non-active pearls
  get a soft `emissive = 0.22` inner glow; cord color = CYAN.
- The opaque 2D hit-test dots + flat 2D edge lines were MASKING the 3D bodies. For the pearl scene they're now
  suppressed to thin rings (dots) / skipped (edges) so the glossy pearls + glowing cords are the graph. Labels +
  hit-testing preserved.
Result: nodes read as translucent prismarine beads on a luminous teal cord — the fiber-optic array.
STILL OPEN: inscribed text ON the pearl surface (currently floating labels); stronger prismarine sheen tuning;
and the procedural extent/architecture so the 136-node strand sits inside the dome instead of clipping out left.
