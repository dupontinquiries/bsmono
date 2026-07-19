# LANES — concurrent generation over one resident model

Two `:agent`/`:fan-agent` tasks used to QUEUE: the app's `Inference` is one worker thread over one
backend, so the fan's "parallelism" was round-robin time-slicing. The engine never had that limit —
weights are read-only after load (RULESET K9) and `serve.rs` already runs many threads against one
`Arc<Model>`. Lanes bring that to the app.

## Using it

```
:lanes 2            # spin 2 lanes over the SELECTED local .gguf (loads once, ~seconds)
:fan-agent <task>   # branches now genuinely decode AT THE SAME TIME
:lanes off          # back to the single worker
```

- One resident weights copy, shared read-only across lanes (`Engine::lane()` clones `Arc`s — no GGUF
  re-read, no dequant, no pipeline rebuild). Per-lane cost ≈ one KV cache.
- Lane jobs stream tagged events (`lanes.rs::LaneEvt`) — the chat routes them to fan branches by job
  id, so the single `Inference` worker stays free for the conversation itself.
- Decode is bandwidth-bound: concurrent lanes ride the same weight reads (the `run_batched` effect,
  without forcing tasks into one lockstep batch). Expect aggregate tok/s ↑, per-lane tok/s slightly ↓.
- `:stop` cancels lane jobs too. Lanes carry no submit tuner (the primary tunes the shared
  `flush_every`) and no draft model.

## Submit governor (display stutter fix)

`Gpu::submit` used to fire command buffers back-to-back — ~24–28 submits/token stacked in front of the
compositor on Asahi. Now at most **2** submits are in flight (completion-pumped, non-blocking); the
rest wait a few µs. Bit-exact: grouping/pacing only, no kernel or ordering change.

- `LOGSCOPE_SUBMIT_INFLIGHT=N` — inflight cap (default 2, `0` = off).
- The flush tuner is now **budget-aware**: it settles on the fastest candidate whose per-submit cost
  stays under `LOGSCOPE_SUBMIT_BUDGET_MS` (default 8 ms, `0` = pure speed), else the smoothest. The
  settle log prints ms/tok *and* ms/submit per candidate.
- On-device check: `gpu-gemm --gate-submit` (kernel-validate includes the budget rules).

## Patch-first agent + verify digest

- New `edit` tool: `{"tool":"edit","path":…,"find":"<verbatim unique anchor>","replace":…}` — applies
  exactly once or fails with a teachable error (0 matches → re-read; >1 → grow the anchor). The system
  prompt now tells models to patch existing files instead of re-emitting them
  (`AgentConfig.prefer_patches`, default on).
- Verify output is folded as a **digest** (counts + top `error — file:line` list + first error's
  detail) instead of the raw 64 KB dump (`AgentConfig.digest_verify`, default on). A Delegate-slot
  model can rewrite the digest conversationally later; the loop never depends on one.

## Cargo tab

Point it at any Cargo root: streaming `cargo check --message-format=json` diagnostics (expand /
explain-in-chat / copy), **Init .gitmind here** to make the root loadable, **Work on this in Chat**
(prefills `:load-gitmind <root>`), and a bounded file-peek list that can hand any file to Chat as
context.
