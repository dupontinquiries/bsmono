# 11 · The hive: workers, the model matrix, and where KV warmth belongs

*`capability.rs` and `lib.rs` both cite `HiveMind_Strategy_Logscope.md` as the source of a
mandatory requirement. That file is not in the tree — the citation outlived it. This document
is the design as it actually stands in the code, plus the axis that was missing, so the next
reader has something to check the code against instead of a dangling reference.*

## The shape

A hive is a set of **cells**. One cell is a model resident on a machine, holding whatever KV
state its last job left behind. The matrix is every cell the coordinator can currently reach:

```text
                 m2ultra (0ms)      mini (30ms)       api (150ms)
  Qwen3-14B      warm 1600 tok      cold              —
  deepseek-6.7b  cold               warm 300 tok      —
  draft-1B       cold               —                 —
```

Placement is the question "which cell runs this job", and it has four inputs. Three were
already implemented, separately and correctly, and nothing joined them:

| Axis | Module | What it decides |
|---|---|---|
| Role / reach | `workerkit` | whether the network is allowed **at all** |
| Identity | `capability` | what a worker IS, so it can be told its job |
| Breadth | `coordinator` | how many cells to light up (barebones → aggressive) |
| **KV warmth** | *was nowhere* | which cell can skip the prefill |
| Verb | `continuation` | whether to keep going, fan, check, or stop |

`hive.rs` is the join. It is pure — facts in, placement out — for the same reason
`continuation` is: a scheduler that costs a round trip to make a decision is a scheduler that
gets switched off.

## Why KV warmth is an axis and not a detail

Prefill is the expensive pass. From a field log: a 1625-token first turn spent **66 seconds**
before emitting a token, then decoded at 3 tok/s. The engine already reuses prefixes — the
prefix tree picks the pool entry sharing the longest prefix (`kvtree::pick_anchor`, pool cap 3,
see `08-kv-cache.md`) — but that reuse was a *local* optimisation. Across machines nobody asked
which box was already warm.

The consequence is a placement a naive "fastest node" scheduler gets exactly backwards:

```text
  m2ultra   60 tok/s, cold        1700 tokens to prefill
  mini      20 tok/s, 1600 warm    100 tokens to prefill
```

The mini is three times slower and finishes first. `hive::place` scores prefill plus round trip
rather than throughput, so it picks the mini — and `hive.rs`'s first test is exactly this case,
because it is the one the obvious implementation fails.

**Warmth is not transferable.** A KV cache is gigabytes and is invalid the moment the prompt
differs; shipping it between machines is not worth considering (`workerkit` says the same). So
warmth is a property of *where a conversation has already been*, which makes it a scheduling
input and never a resource to move.

## Reach is hard, and the arithmetic says why

A speculative draft alternates with its target every few tokens, so a round trip is paid **per
token**, not per turn:

```text
  thinker, 40-token reply     1628 ms gen + 30 ms rtt   =  1.8% overhead
  speculative draft, /round     50 ms gen + 30 ms rtt   = 37.5%
                                50 ms gen + 150 ms rtt  = 75.0%
```

So `Reach::Colocated` is a filter applied before anything else in `place`, and no amount of
warmth or speed on another node can override it. Fan-out is the opposite case: independent
branches each generate a whole reply before anything is compared, so they amortise like a
thinker and a second machine is genuinely useful.

When placement fails, `explain_empty` names which of three things happened, because they call
for different operator actions:

* **all cells busy** — a wait, not a failure;
* **no cell runs the required model** — configuration, not load;
* **the partner's node has no free cell** — a co-residency squeeze that adding remote machines
  cannot fix.

## What is deliberately not here

* **No live topology.** `hive.rs` takes a matrix; building it from `discovery`/`clusternode`
  is the caller's job and involves I/O.
* **No preemption.** A busy cell is skipped, never interrupted. Interrupting a warm cell to
  place a job that then has to prefill somewhere else is a net loss, but proving that needs
  numbers this module does not have.
* **No warmth prediction.** `warm_prefix` is measured by the engine's own anchor pick, not
  guessed from conversation shape. A predicted warmth that is wrong is worse than a cold cell,
  because the schedule commits to it.

## Where to look

* `crates/logscope-fan/src/hive.rs` — the matrix and `place`, with the tests that pin the
  warmth and reach rules (both break-checked: removing either rule fails two assertions).
* `crates/logscope-fan/src/workerkit.rs` — the latency arithmetic, in full.
* `crates/logscope-fan/src/capability.rs` — the per-worker contract.
* `docs/08-kv-cache.md` — what a KV cache is and how the prefix tree reuses one.
