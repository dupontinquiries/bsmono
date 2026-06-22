# Tooling: calling and piping binaries at scale

This lays out how the maintenance assistant turns model output into executed work — an
FSM bot decides, the engine runs binaries (and pipelines of them), and the results pipe
back into the model's context. The executor exists today as `gpu_kernels::toolpipe`
(std-only, validated in `kernel-validate`); this document is the surrounding pipeline.

## The loop

```
            ┌──────────────┐   tool call (text)   ┌──────────────┐
   prompt → │    model     │ ───────────────────▶ │   FSM bot    │
            │  (decode)    │                       │  decides     │
            └──────────────┘                       └──────┬───────┘
                   ▲                                       │ Action::Call(Pipeline)
        observation│  (stdout/stderr, capped)              ▼
            ┌──────┴───────┐    PipeOutcome        ┌──────────────┐
            │ context pipe │ ◀──────────────────── │  toolpipe    │  spawn → wire pipes →
            │ (bounded)    │                       │  run_pipeline│  cap output → kill on timeout
            └──────────────┘                       └──────────────┘
```

One turn: the model emits text; the bot parses tool calls from it and steps its state
machine; each `Call` runs a `Pipeline` through `run_pipeline`; the `PipeOutcome` becomes
the next `Observation`, which the bot folds back into the prompt. Repeat until the bot
emits a final answer (`Done`) or gives up (`Fail`).

## Layers (bottom-up, all in `toolpipe`)

1. **`Stage` / `Pipeline`** — a binary + args, chained with OS pipes. `Pipeline::single("journalctl", &["-u","nginx"]).pipe("grep", &["ERROR"])` is `journalctl -u nginx | grep ERROR`. Optional `stdin` is fed to the first stage on a writer thread (no deadlock on big input).
2. **`Policy`** — the safety gate: an allowlist of permitted binaries, an output cap (bytes), and a timeout. Every binary in every stage is checked before anything spawns.
3. **`run_pipeline`** — spawns the chain, wires each stdout into the next stdin, captures the last stage's stdout (capped, with a `truncated` flag) and stderr, waits for exit codes, and kills the whole chain if it runs past the timeout. No shell: args are a vector exec'd directly, so there is no `sh -c` and no shell-injection surface.
4. **`ToolRunner`** — N worker threads draining a job queue, each sharing one `Policy`. This is the "at scale" layer, and it is deliberately the *same shape* as the model fleet (Arc-shared state, threaded fan-out, mpsc results), so a single scheduler can interleave tool calls and decode against one budget.
5. **`Bot` / `drive`** — the FSM contract. A bot implements `step(&Observation) -> Action` where `Action` is `Call(Pipeline) | Emit(String) | Done | Fail`. `drive` runs the loop, executes `Call`s, and feeds each outcome back as the next observation, bounded by a step budget.
6. **`parse_calls`** — a reference parser turning `!run …` lines of model text into pipelines. Swap in your own parser; the rest of the pipeline is unchanged.

## Wiring your FSM bots

Your state machines only need to implement `Bot`. The integration points:

- **Decoding → decision.** Generate with the engine (`generate_kv_stream`), then hand the
  text to the bot. The bot owns parsing (use `parse_calls` or your own format — JSON tool
  blocks, a grammar-constrained decode, etc.). The executor is parser-agnostic.
- **Decision → execution.** Return `Action::Call(pipeline)`. `drive` runs it under the
  policy. For many independent calls in one step, submit them to a `ToolRunner` instead and
  collect results — that is the fan-out path.
- **Execution → observation.** The `PipeOutcome` (stdout/stderr/code/truncated) is the next
  `Observation`. The bot decides what to splice into the next prompt.

## Piping results back without blowing the context

Tool output is often huge (a `journalctl` dump). Two bounds keep it model-sized:

- **At capture:** `Policy.max_output` truncates at read time, so a runaway command can't
  fill memory; `truncated` tells the bot it was clipped.
- **At context time:** before splicing into the prompt, summarize with the same
  `repeated_templates` pass the chat log-injection uses (line count + top repeated patterns
  + head/tail), so 50k lines become a few hundred tokens the model can actually reason over.

## At scale

- **Shared scheduler.** `ToolRunner` and the model fleet are the same concurrency shape, so
  one scheduler can hold a budget across both: e.g. cap total threads, prefer decode when a
  user is waiting, drain tool jobs in the gaps. The fleet's largest-first / trie prefix
  sharing has no analogue for tools, but the worker-pool + queue + results channel is identical.
- **Backpressure.** The job queue is the natural throttle; bound submissions per bot so one
  runaway FSM can't starve the others.
- **Isolation.** Each pipeline is its own process group; a timeout kills the chain. Per-bot
  policies (different allowlists) let you sandbox untrusted bots harder than trusted ones.

## Security posture

- **Allowlist, not denylist.** Nothing runs unless its binary is explicitly permitted.
- **No shell.** Direct exec with vector args; no globbing, no `&&`, no injection.
- **Caps + timeout + kill.** Output bounded, wall-clock bounded, chain killed on overrun.
- **Auditability.** `Pipeline::display()` gives the exact `a | b | c` string for logging
  every command a bot ran — pairs with `netserve`'s request log for a full trail.

## Open / next

- A real scheduler object owning both the fleet and a `ToolRunner` with one budget.
- Streaming outcomes (for `-f` follow commands) instead of run-to-completion — a `Stream`
  variant of `PipeOutcome` carrying a channel, plus an FSM action to cancel.
- A richer `parse_calls` (quoting, env, redirection) or a JSON tool-call schema with a
  grammar-constrained decode so the model can't emit a malformed call.
- Per-bot resource accounting (cpu-time, output bytes) surfaced to the scheduler.
