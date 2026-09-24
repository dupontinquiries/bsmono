# logscope-hive

The multi-box session core. Headless; `hive` is the CLI over it and the egui shell is the next consumer.

| module | what it is | where the rule came from |
|---|---|---|
| `directive` | the JSON both boxes share: status, models, load, unload, run, summary, say, integrate (a base64 git bundle → verified ref → worktree → verifier → merge advice) | the bubble needs an exit that is a ref, not a merge |
| `endpoint` | one streaming client for OpenAI SSE, Anthropic Messages SSE, Ollama NDJSON, and Claude Code `-p --output-format stream-json` | the Max plan may only be used by driving the real `claude` binary |
| `matrix` | boxes with roles (worker / verifier / integrator), per-box concurrency, measured TTFT and tok/s, persisted at `~/.config/logscope/matrix.json` | "many boxes probing cheaply, one place integrating" |
| `plan` | tasks as a DAG with declared `deps`, a `forward` contract, premise checks, and an optional verifier | declared independence; summary contracts; extract don't derive |
| `dispatch` | topological waves; a wave fans out across every box holding the role, gated by concurrency; premise checks before any box's time is spent; dependents receive only the forwarded summary; every result and verdict goes to the ledger | the activations-to-weights asymmetry |
| `ledger` | append-only JSONL with `result`, `verdict`, `premise`, `note`, and `absence {looked_for, did_not_find, command, scope}`; `recheck` re-runs the absence commands | the caste that is never staled; the check is the negotiation |
| `counter` | fan-out / serial / single turns and read / verify / build / run kinds over Claude Code transcripts, plus verify-before-build and diagnose-after-error | measure before designing around the 60/40 guess |

```
hive matrix add ultra openai http://ultra:8080 --model default --roles worker,integrator --np 4
hive matrix add laptop anthropic http://127.0.0.1:8080 --roles worker,verifier
hive matrix add mini ollama http://mini:11434 --model qwen3.6:35b-a3b --roles worker
hive matrix add claude claude - --roles integrator          # drives `claude -p` on this machine
hive matrix probe                                            # health + a 16-token smoke run per box
hive gen ultra "prompt" --max 256
hive example-plan > plan.json && hive run plan.json
hive absence --looked-for "AV1 decode on M2" --did-not-find AVD --command "vulkaninfo --summary" --scope "asahi 6.19"
hive recheck; hive ledger 20
hive count ~/.claude/projects/<slug>/<session>.jsonl
```

Build note: `apps/logscope/.cargo/config.toml` sets `-C prefer-dynamic` for iteration speed, which conflicts with the
workspace's `lto = true` in release. Build with `CARGO_PROFILE_RELEASE_LTO=false cargo build --release -p logscope-hive`
(or from a dev profile), and run with the toolchain's `lib/` on `LD_LIBRARY_PATH` as every prefer-dynamic binary in this tree already needs.

Verified here: everything above against fake OpenAI, Anthropic and Ollama servers (streaming, timings, draft acceptance,
forwarding contract, premise check, verifier, ledger, recheck, counter, stream-json parsing). Not verified: a live
`claude -p` run (no Claude Code in the sandbox); the flags used are `-p --output-format stream-json --verbose
--include-partial-messages [--append-system-prompt] [--resume]`, check them against your installed version's `--help`.

## The `Remote` backend in logscope-egui

`BackendKind::Remote` (crates/logscope-egui/src/backend.rs) is a thin client over `hive::endpoint`. Configure it with
one environment variable and the app holds no weights:

```
LOGSCOPE_REMOTE=openai:http://ultra:8080            # llama-server (MTP if it was started with --spec-type draft-mtp), optiq
LOGSCOPE_REMOTE=anthropic:http://ultra:8080         # the same servers on their Anthropic endpoint
LOGSCOPE_REMOTE=ollama:http://mini:11434/qwen3.6:35b-a3b
LOGSCOPE_REMOTE=matrix:ultra                        # a box from ~/.config/logscope/matrix.json (matrix: = first healthy worker)
LOGSCOPE_REMOTE_NP=4                                # fan-out width for :fa/:ca against that server (default 2)
```

Rules it follows: an explicit, reachable `LOGSCOPE_REMOTE` is preferred over a live unix daemon, which is preferred over
the compiled-in backend (`preferred_reachable`); an unreachable remote falls through silently, like a missing daemon;
`load()` fails fast and names the target; cancel drops the connection, which is what stops every server here;
`generate_batched` fans out across the server's slots in threads; `template()` is Plain because the server applies its
own chat template. Verified: `cargo check -p logscope-egui --features gui` clean, and `backend::daemon_tests` (7 tests,
including the new remote test) pass. Known gap: the chat tab sends its Plain transcript as one user message; passing the
structured history as `messages` needs a `generate_chat` path on the `Backend` trait. Also noted while checking: the
crate's `--no-default-features` (no-gui) build is broken independently of this change (`scene3d.rs` imports `egui`,
`mesh3d`, `texgen` without a `gui` gate).

## The frontier route: policy, not a row

A frontier model is not a fourth row in the matrix; it is a *tier* on a box plus a *route* on a task. Three
axes, each independent:

| axis | lives on | values |
|---|---|---|
| role (caste) | box | worker / verifier / integrator |
| tier and locality | box | small / large / frontier; local / external |
| function and route | task | plan / design / guide / build / verify / review / integrate; `min_tier..max_tier`, `allow_external`, `escalate_on_fail` |

The router picks the cheapest tier that satisfies the route, so nothing reaches the frontier by default.
Two ways it does: a task sets a frontier floor (a design note, a guide, a plan), or a task sets
`escalate_on_fail` and its local attempt is DISCARDed by the verifier, in which case it re-runs once on the
next tier the route allows and the ledger records the `escalation`. Device data and private code carry
`allow_external: false` and can never route off the mesh.

Plans, designs, guides and reviews are stored whole in `~/.config/logscope/artifacts/` and injected with
`{{design:ID}}`, `{{guide:ID}}`, `{{plan:ID}}`, `{{review:ID}}`; everything else forwards only its summary.
That is the glue between the layers: the frontier writes the guide once, the guide becomes the workers'
prefix, worker results come back as summaries, and a review (local first, frontier on failure) closes the loop.

`hive matrix add claude claude - --roles integrator` infers frontier/external; `--tier` and `--locality`
override. `hive example-plan` shows a design → guide → workers → review plan exercising all of it.
Verified against fake small/large/frontier boxes: routing by tier, locality filtering, design injection into
the guide, verifier-triggered escalation from the small box to the large one, artifacts on disk.
