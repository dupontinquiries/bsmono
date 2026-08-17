# Handover — bundles c4-762 … c4-848

Written for a fresh session with no memory of this one. Everything here is either measured or
labelled as unmeasured. Where a claim has a number, the number came from a run.

**Read `docs/03-speculative-and-batched-decoding.md` before touching speculative decoding**, and the
matching numbered doc before touching anything else. This session cost several bundles to
rediscovering things those docs already said — see "How this session wasted time" at the end.

---

## 1. The headline empirical finding

Decode on the operator's M2 Ultra (Asahi/Honeykrisp Vulkan) is **not** bandwidth-bound, not
GPU-execution-bound, and not sync-bound. It is **CPU command recording**:

```
setup 15ms + encode 3534ms + submit 147ms + sync 351ms  of  4183ms   (96.7% accounted)
```

**Encode is 84% of decode wall time — 292 µs per dispatch across 12 095 dispatches** (367 µs on a
longer run). Normal command recording is single-digit microseconds. Inside `record_dispatch` that is
a frame mutex, `begin_compute_pass`, two keep-alive heap allocations and two `Arc` clones.

Everything else measured on that machine follows from it:

| Measurement | Value | Bundle |
| --- | --- | --- |
| Decode throughput, Llama-3.2-1B Q4_K_M | 16.2–17.0 tok/s | log |
| Fraction of the bandwidth roofline | **3%** | c4-808/809 |
| Decode vs this model's own prefill | 15× slower per token | c4-816 |
| Submits forced by pass boundaries | 96% | log |
| Dispatches per layer-token (fused) | **12** | c4-834 |
| Dispatches removed by the c4-820/821/831 fusions | **6 per layer** | c4-834 |
| Decode window size (k) effect on dispatch count | **none at all**, k=1…8 | c4-836 |

The last row deserves attention: the governor tunes the decode window every turn, narrates it, and
it does not change the dispatch count. Either it only affects sampling or it never reaches the
forward path. **Unresolved.**

---

## 2. What broke the operator's repository summary

Symptom: load the monorepo, ask for a summary, get 8 turns, 0 files, stop-loop. Three causes, all
found in one log, two fixed.

1. **The repo was being deleted from its own prompt.** Context grew 6662 → 8115 tokens across the
   agent loop while the window stayed at 4864, so every turn elided up to 3289 tokens *from the
   middle* — which is where the imported repository sat. The model answered about code it had never
   been shown. (`c4-842`)
2. **The prefix cache could never hit.** Every turn: `SuffixTooLong { shared: 1008, suffix: 3830,
   max_suffix: 256 }`. One hit in the whole session; eight full 15.3 s prefills. The 256 cap was a
   chat-sized assumption and an agent turn appends kilobytes. (`c4-843`)
3. **A refused tool call looked identical to no tool call.** `parse_tools_reported` existed since
   c4-801 with **zero callers**, so nothing distinguished "the model tried and the parser refused"
   from "the model emitted prose" — including the loop guard, which signed both as `noop`.
   (`c4-845`)

**Still open:** whether the model emits usable tool calls at all. That is now diagnosable rather
than guessable — the next run will say.

---

## 3. TODOs, in priority order

### T1 — Wire automatic context sizing (small, blocking the repo-reading use case)
`ctxwin::plan_ctx` / `kv_bytes_per_token` / `ctx_reason` are written and tested (c4-848) and **not
wired**. The blocker is one missing read: the loader never reads `{arch}.context_length`, so
`trained_max` has no source.

- read the trained context at load; store on `Model`
- compute the KV budget from the VRAM figure the loader already has
- replace `let window = (p.n_ctx as usize).clamp(1024, MAX_CONTEXT)` in `backend.rs` with
  `plan_ctx(prompt + reply_budget, p.n_ctx, trained, kv_per_token, budget)`
- log `ctx_reason` so a resize says which bound stopped it

Do NOT tell the operator to raise `n_ctx` by hand. That was c4-842's prescription and it is wrong
for a long-horizon agent.

### T2 — Reduce per-dispatch encode cost (largest available win)
84% of decode. Candidates, cheapest first:

- the two `Box::new(arc.clone())` keep-alives per dispatch — typed vecs would remove both allocations
- the frame mutex taken per dispatch
- `begin_compute_pass` per dispatch. **Pass fusion is RETRACTED** (`LOGSCOPE_PASS_FUSE`) — it broke AR
  decode 6/6 in c4-736 and changes tokens on 3 of 6 diffusion configs. Do not re-enable it without
  reading `tools/gate-passfuse-diffusion.sh`. Within a WebGPU compute pass there is no barrier
  between dispatches, which is very likely *why* it corrupts — so a correct version can only fuse
  dispatches with no data dependency.

Measure with `count-dispatch` (below) plus the operator's `encode` term. Do not trust a diff.

### T3 — Identify the 27.9% kernel
`LOGSCOPE_DISPATCH_HIST=1 count-dispatch <model> <n> <k>` reports per-shader shares. One kernel is
**4.00 per layer-token, 27.9% of all dispatches** — a norm+matvec (its struct has `eps`). Four more
sit at exactly 2.00. Identify it, then decide whether it fuses.

### T4 — Gated DeltaNet forward pass (unlocks the entire current model line)
Every recent Qwen is hybrid recurrent and refuses:

| Model | ssm blocks | total |
| --- | --- | --- |
| Qwen3.6-35B-A3B | 30 | 40 |
| Qwen3.6-35B-A3B-MTP | 30 | 41 |
| Qwen3.8-27B-MTP | 48 | 65 |

A DeltaNet block **reuses the attention projection names** for its own q/k/v, which is why the naive
loader reported a qkv row-count mismatch far downstream. `gguf::ssm_layer_census` detects it and
refuses early (c4-810). This also gates MTP: every MTP-bearing checkpoint the operator has is hybrid.

### T5 — MTP speculative decoding
`gguf::mtp_census` detects the draft head (`blk.N.nextn.*`, `{arch}.nextn_predict_layers`) and reports
it as present-and-unused (c4-828). The verifier already exists and is proven; MTP is a new `Drafter`
impl, not a new subsystem. Blocked on T4.

### T6 — Gemma 4 (the one recent model that is NOT hybrid)
Conventional transformer MoE, so it is reachable. Three named gaps, all in the refusal text:
per-layer head dimension (SWA layers differ from full-attention ones — flagged STRUCTURAL), per-layer
learned output scale (`blk.N.layer_output_scale.weight`, present and ignored), attention scale from
hparams. Real tensor dumps for 26B-A4B and e2b are in this session's logs.

### T7 — Smaller, known
- `kernel-validate --gate-qwen3` FAILS on both fused and non-fused decode_step against the CPU
  reference. **Pre-existing**, verified by disabling the c4-821 fusion and rerunning. Not in the
  routine battery, which is presumably why nobody noticed. Could be a stale gate or a real bug.
- MLA layer loading is not implemented; the branch derives dimensions, logs, and ends (c4-799).
- Q5_K is the only common K-quant with no slice path. Whether it matters is measurable from the
  `load path` percentage.
- The UI diagnostics work (Engine page, banner, model-switch markers) is **not visually verified** —
  c4-822 … c4-827 were all written blind.

---

## 4. Tools built this session — use these instead of reading diffs

| Tool | Answers |
| --- | --- |
| `tools/egui-check.sh` | PASS/FAIL/RUNNING for the GUI crate. **Use this**; a background `cargo check` log with no errors yet is indistinguishable from a clean one, which shipped a broken build in c4-839. |
| `count-dispatch` (bin) | Dispatches per token/layer, per-shader histogram, and the generated tokens. Device-independent. |
| `LOGSCOPE_NO_DECODE_FUSE=1` | A/B any decode fusion in the same binary. |
| `tools/counter-scope-check.sh` | Counters that are both reset and baselined, or baselined at a literal zero. |
| `tools/unwired-check.sh` | Public fns with no non-test caller. Noisy (~728); use `\| grep my_new_fn`. |
| `tools/kvtree-check.sh`, `physui-check.sh`, `diagfeed-check.sh`, `convoprov-check.sh` | Pure modules in ~1s each, without the 12-minute GUI build. |
| `tools/quantize-tiny.py` | Re-emit a fixture in Q8_0 so it reaches the quantised paths. |
| `tools/make-q4k-probe.py` | Real Q4_K super-blocks for transcode tests. |

Battery: `cargo test --release --lib` in gpu-gemm (952), the four `*-check.sh`, `tools/oracle/run.sh`,
`run-diffuse.sh`, and `egui-check.sh`.

---

## 5. Research findings worth not re-deriving

**Speculative decoding already existed** before this session (`speculative_generate_stats`, opt-in via
`LOGSCOPE_SPEC_DRAFT`), and `docs/03` documented its equivalence. c4-829 built a drafter seam
believing the loop was missing. The seam turned out useful for a different reason — drafting required
a second checkpoint — but the loop was never the gap.

**Prompt-lookup drafting is worth ~1.5× at best**, and nothing on prose. Acceptance depends only on
the token sequence, so it is simulable without a GPU:

```
corpus                    ng=2   ng=3   ng=4      (tokens per verification pass, k=4)
repo-read (long prompt)   1.49   1.37   1.27
rust source               1.32   1.22   1.15
prose                     1.04   1.02   1.01
```

Two tunings were simulated and **rejected**: variable-order n-grams gain nothing on real text (only on
synthetic periodic sequences), and widening k past 4 is nearly free and nearly worthless (1.49 → 1.55
at k=16 while acceptance falls 12% → 3%). Both are recorded with guard tests in `spec.rs`. The limit
is what the drafter can *know*: it predicts what the context already said, and generation stops
echoing its context after a token or two. Past ~1.5× needs a drafter that models the target.

**The roofline needs no probe.** Decode reads each active weight once per token, so achieved bandwidth
falls out of throughput. Validated against a published DGX Spark benchmark (143.7 vs our 144). Two
traps: price an MoE on ACTIVE parameters (total under-predicts 7×), and include the per-32 scale —
every published ceiling quoting 0.5 bytes/param is optimistic by 20%.

**Model landscape.** Qwen3.5 onward in the MoE line is hybrid recurrent and unrunnable here. Gemma 4
26B-A4B is a conventional transformer MoE (3.8B active of 26B) and is the most plausible next target.
Qwen3.5-122B-A10B is the largest thing in that line that would fit 192 GB — if it were not hybrid.

---

## 6. How this session wasted time — patterns to avoid

Recorded because each cost more than one bundle.

1. **Reading the code instead of measuring it.** Three times a plausible reading of what runs was
   wrong: `wqkv` already existed, `LinKind::Q8` was already wired, speculative decoding was already
   implemented. `count-dispatch` and the histogram exist because of this.
2. **Verifying an edit against my own pattern.** c4-820 replaced three of four sites, reported
   "unfused left: 0" against the pattern it had just used, and missed the site that actually runs. The
   one that mattered wrote `add_t(&x, &o)` instead of `add_t(x, &o)`.
3. **Reading an incomplete build log as a result.** c4-839 shipped a broken GUI build this way.
   `egui-check.sh` now owns the waiting.
4. **Capabilities built and never called** — four instances (`parse_tools_reported`, the encode timer,
   a kernel in a file that was not a module, a fused add). Tests passed because tests call directly.
5. **Asserting a rounded number.** Twice I asserted a power of two computed in my head and the code
   was right both times.
6. **Numbers with invented denominators** — five in one session (892957502× phase balance, 265600%
   cache rate, 0.5 dispatches/pass, 22090 cacheable, 269× phase balance). `counter-scope-check.sh`
   catches one class.

The through-line: **the claims written in comments were consistently less reliable than the fixtures
written beside them.** Where a test and a belief disagreed this session, the test was right every
time.
