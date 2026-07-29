# Merge audit: monorepo-338 ← session work

Two histories diverged from `a8cd4e4`: **105 commits** on the uploaded bundle
(`20260719--monorepo-latest_338`) and **91 commits** from this session. Both are
preserved as branches; `merge/monorepo-338` is the reconciliation.

| branch | what it is |
|---|---|
| `incoming/monorepo-338` | the uploaded bundle, untouched |
| `session/local-91` | this session's work, untouched |
| `merge/monorepo-338` | the merge, with the resolutions below |

## What each side touched

Theirs: **only `apps/logscope`** (gpu-gemm inference + logscope-egui).
Mine: logscope scenes, plus beatspace, fao-codec, gitmind, rusty-board,
snapclone, studio, tomescope, voiceterm, blackstar, clawbound, paperbound,
soldier, unendlich, maintenance.

Overlap was **11 files**, all under `apps/logscope`. Everything else merged
without contest, so no work outside logscope was at risk.

## Resolutions, and why

**`crates/gpu-gemm/` — taken wholesale from 338.** Their 105 commits are almost
entirely this crate and build on each other; my contribution to it was a single
early commit. Blending produced a tree that compiled in pieces but had two
independent fixes for the same frame-reopen bug, which is worse than either.

Superseded from my side, recorded so nothing is lost silently:

* `SUBMIT_CAUSES` / `submit_cause()` — a six-way breakdown of *why* each submit
  happened. Theirs has `SUBMIT_BY_DOWNLOAD`, which covers the same diagnosis at
  lower granularity. **Capability retained, detail reduced.** Re-adding the
  breakdown on top of their frame logic is a small, self-contained follow-up.
* `embed_table_t` — my gemma3 embed-scale path. Theirs solves the same problem
  as `scale_embed_t`, which is present and used. **No loss.**
* My `archcfg` folding of `glm-dsa` into the `deepseek2` arm was **wrong** and
  theirs is right: `prefix` selects the metadata-key namespace, so a `glm-dsa`
  file spells its keys `glm-dsa.attention.*` and the folded lookup reported
  "no kv_lora_rank" about a checkpoint that states it plainly. Their split entry
  is the correct fix and is kept.

**`MoeConfig::scale` — reconciled rather than dropped.** My side added a routed
scaling factor (`expert_weights_scale`: DeepSeek-V3 2.5, GLM 1.8); their
`moe_config` predates the field. Rather than delete the feature or break their
callers, `moe_config` now defaults `scale: 1.0` — which reproduces V2 behaviour
exactly, i.e. what that signature did before the field existed — and
`moe_config_scaled` carries the metadata-driven value. Both sides compile; the
capability stays reachable.

**Additive conflicts — both sides kept.** Nine of the twelve hunks were two
different additions at the same line (tests, comments, telemetry). Both were
retained; none were dropped for being inconvenient to merge.

## Verification

* `gpu-gemm` compiles.
* `logscope-egui` **637/637** — up from 623 on my side alone, so their 14 new
  tests and my scene tests all pass together.
* `part_kit` scene tests (3) pass, confirming this session's logscope work
  survived the merge.
* This session's other work is present and untouched: `apps/studio` (seven
  crates), `rusty-board/egui-frontend` dictation, snapclone, voiceterm, gitmind.

## Follow-up worth doing

Re-apply the granular `submit_cause` breakdown over their frame logic. It is the
one place where the merge traded detail for coherence, and it was deliberate.
