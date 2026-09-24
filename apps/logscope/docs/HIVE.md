# HIVE — how a session works when more than one box is in it

Distilled from three analyses (this chat, the "hector" chat, and the Asahi corpus mining) into rules the code enforces.

1. **The thing that crosses a boundary must be small.** Pipeline parallelism works because activations are KB and
   weights are GB. The engineering translation is not "batch the tool calls" but: find where the dependency payload is
   small relative to the work. `plan::Forward` is that payload, declared per task and enforced by `dispatch`.
2. **You can schedule a known graph; you can only instrument a discovered one.** Fan-out is declared (`deps`), never
   inferred. Discovered dependencies stay serial, and the lever there is making each observation cheap (an instrument),
   after which speculation over the top-k hypotheses becomes affordable.
3. **Castes.** Workers are cheap and staleable wherever a compiler, test or measurement judges the output. Verifiers run
   *before* commitment (premise checks: extract the constant from the tool, never derive it). One integrator holds the
   boundary graph and is expensive; its bottleneck is context, so it lives on a cached prefix. The ledger is never staled.
4. **Absence is evidence.** `looked_for / did_not_find / command / scope`. A negative result carries the command that
   re-checks it, so it goes re-runnable instead of stale.
5. **Boundaries are where systems fail and tests don't fire.** Make them assertable (frame-thread budgets, submit-length
   watchdogs, network turn timeouts) so more failures become environment-judged and more work becomes worker-shaped.
6. **Safety is the scaffold.** Device write tools live only in small deterministic tested workers behind a trap layer
   (allowlists, dry-run, apply, verify effect). The frontier model plans and reviews; it holds no device handles.
7. **Measure before designing.** `hive count` on real transcripts gives the fan-out / serial split and the
   verify-before-build ratio; the scheduler is worth exactly what those numbers say.

Where the earlier pieces plug in: llama-server with `--spec-type draft-mtp` (or `ngram-simple`) is a box with roles
worker,verifier; the M2 Ultra is `ultra` with `--np 4`; Claude Code is an `integrator` box driven headless; the perf loop
(`perf-loop/`) is the instrument that makes the wgpu engine's observations cheap; `racer` is the two-lane special case
of `dispatch` with a UI, and its core should migrate onto `hive::endpoint`.

## What is wired now (2026-09-23)

| piece | command / place | what it does |
|---|---|---|
| the Queen | `:survey <question>` in Chat | deterministic outline/grep/slice probes over the loaded workspace (`queen::answer` + `probe_exec`), every miss ledgered as an absence with its re-check command, the reads and the answer emitted as a `hive::plan::Plan` |
| personas | `workerroster` | a fitting persona becomes the read tasks' system prompt; the roster is a task attribute, the matrix is where boxes live |
| running it | Hive tab → Plan → Run on matrix | `dispatch::run` on a thread with a channel `Observer`; waves, deltas and KEEP/DISCARD drawn live |
| the bubble's exit | `:bundle` in Chat → Hive → Integrate | snapshot + `mount::bundle` into `~/.local/share/logscope/exports`, path handed to the mini panel, `Directive::Integrate` lands it as `refs/logscope/<name>` after the verifier |
| tool discipline | agent turns | `GenParams.grammar`: the tool-fence GBNF from the run's whitelist, honoured by llama-server and by in-process llama.cpp; `LOGSCOPE_TOOL_GRAMMAR=0` disables |
| the scoreboard | end of every agent run | `HarnessCounts` line in the note and the ledger, same terms as `hive count` |
| the liaison | every load / turn | one status line out through ledger / shim / DHT |

## Qwen/Ornith specifics (2026-09-23)

Ornith is a Qwen3.5-MoE fine-tune, so it reasons in `<think>…</think>` and ships its own chat template.

- **Thinking is not action.** `workspace_exec::strip_thinking` removes `<think>` blocks before the tool
  parser looks for a fence, so a call the model *considered* inside its reasoning is never run; an
  unterminated block (reply cut off mid-thought) is treated as all-thinking. `template_kit::split_thinking`
  gives `(thinking, answer)` for callers that want to show the reasoning separately.
- **Reasoning on the wire.** `hive::endpoint::Event::Reasoning` carries `reasoning_content` (OpenAI),
  `thinking` deltas (Anthropic) and Claude Code's thinking deltas, kept apart from answer tokens.
- **The model's own template.** `template_kit::gguf::read_templates_from` reads `tokenizer.chat_template`
  (and the `tool_use` variant) straight out of a GGUF's header without loading the model. With
  `LOGSCOPE_GGUF_TEMPLATE=1` the wgpu chat path renders that instead of the hand-built family, which is
  what makes Ornith's template exactly right rather than approximated; off by default, and it falls back
  to the built-in family whenever the GGUF has no template or rendering fails. The tool-fence grammar
  and llama-server's native `--jinja` parsing cover the tool-call side for the same models.

## Structured chat and specs → tasks (2026-09-23)

- **`generate_chat` on the `Backend` trait.** A backend that has the model's chat template applies it
  itself. The default flattens to `generate()` (readable neutral transcript), so the wgpu engine loses
  nothing; `Remote` overrides it, sending real messages to llama-server's `/v1/chat/completions` so the
  server wraps them in the model's own turn markers — the reliable path for Qwen/Ornith tool calling.
  The Chat tab calls `generate_chat` whenever the backend is `Remote`, `generate(prompt)` otherwise.
  Exposed on the worker as `Inference::generate_chat`, plumbed through `Cmd::GenerateChat`.
- **`LOGSCOPE_GGUF_TEMPLATE=1` covers both local engines now.** wgpu and in-process llama.cpp both
  render the model's GGUF template (via `template_kit`) when the flag is on, falling back to the
  hand-built family otherwise. Remote is left on Plain here because it templates server-side — building
  a prompt for it would double-apply.
- **`hive::from_spec`.** A fan `WorkerSpec` becomes a hive `Task`: its tool whitelist drives the
  grammar and the harness budget, its `success` becomes the shell verifier (`cargo test passes` →
  `cargo test`), its owns/constraints/purpose go in the prompt. Contract → task, not contract → box: a
  spec is a job's constraints, a box is an endpoint.

## `--jinja` in Rust: both halves, and who owns which

llama.cpp's `--jinja` does two things. Both now exist in Rust, in different crates, on purpose:

| half | llama.cpp | here |
|---|---|---|
| render: messages + tools → the model's prompt | minja + the GGUF's `chat_template` | `template_kit::Renderer` (minijinja + pycompat) over `template_kit::gguf::read_templates_from`, which reads `tokenizer.chat_template` straight out of the GGUF header |
| parse: reply → `{name, arguments}` | a hand-written parser per family | `template_kit::toolcall::parse`, format detected from the template source; and, already present, `workspace_exec::canonical_block` for the egui harness |

Two parsers is deliberate, not an oversight. `canonical_block` is the egui harness's, predates this, and
already covers Qwen/Hermes, Llama 3 (`<function=…>` and bare JSON), Mistral and DeepSeek/GLM — a test
now pins that so a regression cannot quietly turn a good call into "no tool call". `toolcall` is the
library version for everything outside that crate (the shim, the hive CLI, anything driving a box
directly), and it adds what the harness has no need of: reasoning split, explicit truncation reporting,
DeepSeek's unicode delimiters, and `parse_with_syntax` for a template that matches no known family.

A third path exists and is the best one when it is available: **let the server do it**. llama-server run
with `--jinja` applies the template on the way in and its matching parser on the way out, returning
structured `tool_calls`. `hive::endpoint` now assembles those streamed fragments and emits
`Event::ToolCall { id, name, arguments }`, so a box that speaks OpenAI gives the caller parsed calls with
no client-side pattern matching at all. Order of preference: server `--jinja` → the harness parser →
`toolcall`.

## Constrained decoding on the wgpu engine (all-Rust)

The `--jinja` table above left one gap: llama.cpp and llama-server mask the vocabulary with a GBNF
automaton, and the wgpu engine — which samples in Rust — ignored `GenParams.grammar` entirely. So a
small model on wgpu could still emit a malformed tool block. That is closed now, in Rust, with no C++
in the path:

| layer | where | what |
|---|---|---|
| acceptor | `template_kit::constrain::ToolFence` | an incremental CHARACTER-level state machine for `prose · ```tool {json} ``` · prose`; rejects a non-whitelisted tool name **as it is typed**, tracks strings and escapes so a brace or a fence inside a value closes nothing, refuses a second call, and reports violations rather than hiding them |
| sampler hook | `gpu_kernels::TokenFilter` + `pick_constrained` | the engine asks the filter about its top-k candidates and samples within the legal subset; identical to `pick` (RNG draw included) when no filter is passed, and if every candidate is rejected the unconstrained pick stands and `on_exhausted` fires — a constraint that can deadlock a decode is worse than one that occasionally lets a token through |
| decode loop | `Gpu::stream_session_constrained` (what chat decodes through) and `Gpu::generate_kv_stream_constrained` | both plain names are now wrappers passing `None`, so no existing caller changed |
| turn wiring | `engine::tool_filter` + `grammar_tool_names` | the engine reads the run's tool names back out of the GBNF the harness generated (the quoted literals on its `name ::=` line) and hands them to the acceptor, which enforces the same shape natively; an unrecognised grammar still constrains the fence, and nothing here can make a decode fail |
| tokenizer bridge | `logscope_egui::toolfilter::ToolFenceFilter` | ids → text (cached) → verdict; special tokens are always allowed so a stop token can never be vetoed; counts rejections and exhausted steps for the turn report |

Filtering the top-k rather than the whole vocabulary is what keeps it cheap: the sampler has already
reduced 248k Ornith tokens to `top_k` (40 by default) before the filter sees anything.

## Seeing what the constraint and the template actually do

Two pieces of instrumentation, because both features above are the kind that fail silently.

- **`ToolFenceFilter::report`** → the diagnostics *and* the hive ledger, once per turn, and only when
  the constraint argued with the model: `grammar: 12 candidate(s) vetoed, 2 step(s) exhausted
  (constraint overridden there), one call emitted`. A quiet turn prints nothing, so the signal does not
  drown. **Exhausted steps are the number to watch**: at those the grammar vetoed every candidate and
  the unconstrained pick was used anyway, so the guarantee did not hold there — usually the run's tool
  whitelist is missing something the model needs.
- **`:template`** in Chat reads the loaded model's GGUF template, compiles it, and prints into the
  transcript: the architecture, the probed tool syntax, the detected call format, whether
  `LOGSCOPE_GGUF_TEMPLATE` is on, and the same synthetic system+user turn rendered BOTH ways — the
  model's own template and the hand-built family. Same input on both sides, so only the template
  differs. That is what the flag should be flipped on: a diff you looked at, not a hope.
