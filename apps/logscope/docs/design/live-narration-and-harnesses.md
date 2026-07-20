# Live Narration & Per-Turn Harnesses — Design Doc

Status: **spec only, nothing built.** Three related features, ordered by how much real
signal they read vs. how much they infer. Written against the actual code seams in
`gpu-gemm/src/lib.rs` and `logscope-egui/src/tabs/chat.rs` so it's a build spec, not a sketch.

## 0. Framing (non-negotiable)

This is an **homage to Anthropic's Natural Language Autoencoders (NLAs)** and the companion
J-lens / "global workspace" work — NOT an implementation of them. That distinction is load-bearing:

- **Real NLAs** read a model's *activations* (the numbers in the middle layers) via a trained
  activation-verbalizer / activation-reconstructor round trip, judged by reconstruction fidelity.
  The J-lens reads a Jacobian-derived subspace to surface words "on the model's mind" that it
  hasn't said. Both require training extra machinery against the activations.
- **What LogScope can honestly do** is read the signals it *already computes during the forward
  pass* — MoE routing, sampler distributions, pre-commit logits — and attach words to them. This
  is real internal signal (not just re-summarized output text), but it is NOT activation reading.

**UI rule:** wherever this surfaces, it is labelled as an *NLA-inspired homage* with a one-line
disclaimer and a link to the real research. We never call it "reading the model's thoughts" or
imply activation-level fidelity. A user who has read the NLA paper should nod, not wince.

Reference: Natural Language Autoencoders (Anthropic, May 7 2026) and "A global workspace in
language models" (the J-lens). Links live in the UI tooltip, not hardcoded here.

---

## Tier 1 — Narrate the real math (routing + sampler + "about to say")

The genuinely novel tier: attach words to numbers LogScope already has on hand, no extra model.
Everything here reads signal that exists in the forward pass; the only cost is capturing +
labelling it.

### 1a. Sampler-distribution narration ("confidence / deliberation")

**Signal source:** `sample_at` (`lib.rs:747`) already has the full-vocab logits and the `top_k`
window. `softmax` (`lib.rs:658`) is available.

**What to compute (cheap, per sampled token or every Nth):**
- normalized entropy of the top-k post-temperature distribution → a 0..1 "deliberation" scalar
  (low = peaked/confident, high = flat/several near-tied candidates).
- the margin between top-1 and top-2 probability → a "commitment" scalar.

**Words attached:** a small fixed lexicon mapped from the scalar, e.g.
`decisive` (entropy < .2) · `settling` (.2–.5) · `weighing options` (.5–.8) · `genuinely torn` (>.8).
This is a lookup, not a model call — deterministic and testable.

**Homage link:** this is the behavioral shadow of the J-lens observation that the model evaluates
candidates internally before committing. We are seeing the *distribution*, not the activation, so
we say "the sampler is weighing N close candidates," never "the model is thinking about X."

**Seam / cost:** capture in `sample_at`; gate behind `LOGSCOPE_NARRATE=1` (off by default, zero cost
when off, like `route_trace_armed()`). One extra readback of the top-k logits per narrated token —
so narrate on an interval (every ~500ms of tokens), not every token, to stay off the hot path.

### 1b. "About to say" — the J-lens cheap echo

**Signal source:** the pre-commit full-vocab logits in `sample_at`. The top few *non-chosen*
candidates are literally words the model ranked highly and did not emit.

**What to surface:** "considered but didn't say: {word2}, {word3}" for the current token, sampled on
an interval. This is the closest honest analog to the J-lens "words on its mind" finding, because
these ARE ranked-high words that didn't make it to output — just derived from the output
distribution rather than a Jacobian subspace.

**Caveat to encode in the label:** these are next-token alternatives, not multi-step lookahead. The
real J-lens picks words the model may say "at some point in the near future"; we only see the
immediate next-token ranking. The tooltip must state this so we don't imply horizon-planning we
can't see.

### 1c. Routing narration (MoE)

**Signal source:** the route-trace infra is **already built** — `trace_layer` (`lib.rs:188`),
`route_trace` (`lib.rs:302`), `route_trace_armed` (`lib.rs:291`), and the v1-forcing at
`lib.rs:2070`. `--route-analyze` already turns a trace into a coupling report.

**What to surface live:** which experts dominate the current turn's routing, and whether the pattern
is stable or shifting layer-to-layer. We do NOT claim experts "mean" anything semantic (they don't
have clean labels); we say "routing concentrated on experts {a,b}" or "routing is diffuse this
turn." Honest, and it's real math.

**Seam / cost:** the trace already forces the v1 readback path (a sync). So routing narration is only
appropriate in an explicit "inspect this turn" mode, NOT always-on live decode — otherwise it slows
generation. Spec: a post-hoc "explain this turn's routing" button that re-runs the last turn's
prompt with `route_trace_armed()` on, then labels the trace. Cheap because it reuses `--route-analyze`.

### Tier-1 data contract

A single `NarrationFrame { entropy: f32, margin: f32, alternatives: Vec<String>, ts: Instant }`
produced on an interval by the engine and pushed to the chat tab over the existing `Evt` channel
(same path as `Evt::Token`). The chat tab renders it in the status area next to `activity_status`.
Pure mapping fns (`narrate_confidence`, `narrate_alternatives`) live module-level and are unit-tested
exactly like `summarize_progress`.

---

## Tier 2 — Small resident model writes the status summary

Upgrades the heuristic `summarize_progress` (`chat.rs:276`) into a real one-line status written by a
tiny co-resident model, fed *completed* text + Tier-1 signals. Never blocks main decode because it
only consumes finished tokens.

### Seam

`summarize_progress` is already the single seam — it's called on an interval in the token handler
(`chat.rs:1171`) and its output flows to `activity_status`. The whole point of building it as a pure
`fn(&str) -> Option<String>` last iteration was to make this swap a one-function change.

### Spec

- A small model (0.5–1.5B, e.g. a Qwen3 small) co-resides. LogScope already loads one big model via
  the wgpu engine; a second tiny resident is feasible on an M4 Max's memory (the 17.7GB main + a
  ~1GB small fits).
- Input: last K completed messages (NOT the in-flight partial — avoids racing the main decode) +
  the latest `NarrationFrame`. Prompt: "In ≤10 words, say what this turn is doing right now."
- Runs off-thread on its own session, result posted back via `Evt`. Throttled to the same ~5s
  cadence the heuristic uses.
- **Fallback:** if the small model isn't loaded, `summarize_progress` heuristic stays. The small
  model is a *quality upgrade*, never a hard dependency.

### Cost / risk

- A small-model forward every 5s is cheap relative to the main model. But it competes for the same
  GPU — so it must run at low priority / between main-model flushes, or on a strict token budget
  (≤32 tokens). Spec: hard cap output at 24 tokens, one summary per 5s, skip if the main model is
  mid-flush.
- This is where Tier-1 + Tier-2 compose: the small model can be *given* the entropy/alternatives
  numbers and asked to phrase them, so the status reads "settling on the API shape (was torn between
  two signatures)" — words attached to real math, phrased by a real model. That's the payoff.

---

## Tier 3 — Small model auto-writes a test harness each turn

The strongest idea: per turn, a small model generates a *test harness* for whatever the main model
just produced, runs it off-thread, and splices results back. This is agentic verification, and the
plumbing already exists.

### Seam (already present)

- `stepwise` already extracts `// file:` blocks (`extract_files`, `chat.rs:1194`) and runs
  `cargo test` off-thread at step boundaries (`step_shell`, `chat.rs:1197`), splicing the output tail
  back into the continue (`chat.rs:1213`).
- `VerifyJob` (`chat.rs:374`) + `auto_verify_after_reply` (`chat.rs:5357`) already run a sandbox
  `cargo check` of generated files after a reply.

So the new piece is narrow: **replace "run the tests the main model wrote" with "have a small model
WRITE tests for the code the main model produced, then run those."**

### Spec

1. After a turn produces code (detected the same way `auto_verify_after_reply` detects files), hand
   the extracted files to the small model with: "Write a minimal test harness that exercises the
   public surface of these files. Output only `// file: tests/…` blocks."
2. Materialize those test files alongside the generated code in the existing scratch workspace.
3. Run `cargo test` off-thread via the existing `step_shell` / `VerifyJob` mpsc plumbing.
4. Splice the result tail back — either into the next stepwise continue (if stepwise) or as a
   `VerifyJob`-style status badge (if not).

### Why this is safe-ish

- Runs in the **same sandbox** the current `auto_verify` uses — no new trust surface.
- The small model only writes *tests*, never edits the main code. Worst case a bad harness fails to
  compile, which is caught and reported, not acted on.
- Bounded: one harness per turn, one `cargo test` run, output tail truncated (existing behavior).

### Open question (flag for later, not this doc)

Whether the harness results should *feed back into the main model's next turn* (true agentic loop) or
just be shown to the user (verification display). The former is more powerful and more dangerous —
it lets a small model's test failures steer the big model. Spec defaults to **display-only**; the
feedback loop is a separate opt-in decision.

---

## Build order (when we do build)

1. **Tier 1a + 1b** first — pure functions over data already in `sample_at`, gated off by default,
   unit-testable, no model. Highest signal-to-effort, and it's the honest "words on the math" core.
2. **Tier 3** next — the plumbing is 80% there; it's the most *useful* (real verification) and
   reuses `step_shell`/`VerifyJob` almost verbatim.
3. **Tier 2** last — needs a second resident model and GPU-scheduling care; it's a polish layer over
   the `summarize_progress` seam that already works heuristically.
4. **Tier 1c** (routing narration) is opt-in / post-hoc only, folded in whenever, since it reuses
   `--route-analyze`.

## Invariants across all three

- Everything off by default (`LOGSCOPE_NARRATE`, a settings toggle for the small models). Zero cost
  when off — same discipline as `route_trace_armed()` and `fuse_norm_enabled()`.
- No feature blocks or slows the main decode. Tier-1 narrates on an interval; Tier-2/3 consume only
  completed text and run off-thread.
- Every mapping/labelling function is a pure `fn` with unit tests, like `summarize_progress`.
- The UI never overclaims. NLA-homage label + research link wherever narration appears.
