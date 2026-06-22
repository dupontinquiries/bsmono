# BOOTSTRAP — generating large-scale `.logscope` stress tests

Paste this file into a fresh Claude chat **together with the `logscope-suite` repo**. It primes that chat to
generate big branching test scripts for `logscope-cli` — runs that fan into thousands of nodes and take
hours-to-a-day against a local 8B model. This file is the task spec; the repo is the ground truth.

---

## What you're being asked to do

Generate a set of `.logscope` scripts (and any `.chat.md` seeds they open) that drive `logscope-cli` — a
headless runner for a local-LLM conversation-graph tool — into **deliberately large, deep, branching runs**.
The point isn't size for its own sake: each run should exercise the engine and the graph machinery
(branching, KV-cache prefix reuse, structured decomposition, fan-out, best-region selection) on realistic
coding work, at a scale that surfaces performance and correctness behavior you can only see over a long run.

## Orient yourself first (read these in the repo)

You have no memory of how this project was built — the repo carries that history. Read, in order:

1. **`FIELD_NOTES.md`** — the chronological build log. This is the evolution narrative: what was added each
   session, why, and what each piece does. Read it start to finish; it's the fastest way in.
2. **`docs/`** — design chapters 01–10. Most relevant: `08-kv-cache.md` (why a shared prefix is cheap),
   `09-decomposition.md` (the `:fill` sub-task idea), `10-cli.md` (the CLI + how to run it).
3. **`examples/`** — the existing small scripts `01`–`04`, the `.chat.md` seeds, and `examples/README.md`.
   Your scripts are bigger siblings of these.
4. **Source of truth for the grammar:** `crates/logscope-egui/src/runner.rs` (`run_command` — exactly which
   commands the CLI executes), `src/script.rs` (the file format), `src/vimmode.rs` (how counts/args parse),
   `src/decompose.rs` (what `:fill` does). If this file and the code ever disagree, **the code wins**.

If a git checkout is provided, `git log --oneline` and the file tree complement `FIELD_NOTES.md`.

## The project's thesis (so your tests serve the point)

The bet: how far a **small local model + STRUCTURE** — a conversation graph, decomposition into scoped
sub-tasks, fan-out with cheap samples, verification, and a human in the loop — can go before you need a big
model. Good stress tests lean into that: deep decomposition, wide exploration of real solution space, and
checkpoints that leave inspectable artifacts. (A first real run on an M4 Max with
`DeepSeek-R1-Distill-Llama-8B-Q4_K_M` confirmed the harness and the decomposition idea; see the last
`FIELD_NOTES.md` entry.)

## The `.logscope` format

One step per line: `#` is a comment; `>` or `say ` (or a bare line) is a user turn the model replies to; a
line starting with `:` is a command. Blank lines are ignored.

## The command grammar the CLI runs (the wired subset)

Use ONLY these — anything else is logged "skipped" by the runner:

| step | meaning |
|---|---|
| `> TEXT` / `say TEXT` / bare line | a user turn; the model replies (one new branch off the active leaf) |
| `:ca` / `:Nca` / `:ca N` | continue **every leaf**, fanned N rounds deep (depth N). N ∈ 1..200, default 1 |
| `:fa [W] ['tmpl']` / `:Dfa W 'tmpl'` | **fuzzy-all**: D rounds, each fans **W variants per leaf**. W ∈ 2..40 (default 10), D ∈ 1..200 (default 1). In `tmpl`, `{lastmsg}` → that leaf's text, `{msgid}` → its id |
| `:fill [path]` | decompose a `todo!()`/`unimplemented!()` skeleton (a file, else the last reply's code block) into **one KV-shared sub-task per hole**, fill each, splice back |
| `:assemble` | re-splice the last N replies onto the holes |
| `:best` | report the best-verified subtree (currently a code-block proxy — over-counts; treat as a checkpoint, not truth) |
| `:open PATH` | import a `.chat.md` seed (`## You` / `## Assistant` turns) as the starting tree |
| `:goto N` | set the active leaf to node N (where the next `>` / `:ca` attaches) |
| `:export md\|ascii\|git` | dump the current tree to stdout |

## Branching math — how to hit thousands of nodes

A `:Dfa W` stage **multiplies the leaf count by W each round**. From `L` leaves, after `D` rounds you have
`L · W^D` leaves, having generated about `L · (W + W² + … + W^D)` reply-pairs. Each "node" is a user+assistant
pair, so node count ≈ `2 ×` that. From a single leaf:

| stage | reply-pairs | ≈ nodes |
|---|---|---|
| `:3fa 8` | 8+64+512 = 584 | ~1,200 |
| `:4fa 6` | 6+36+216+1296 = 1,554 | ~3,100 |
| `:5fa 5` | 5+25+125+625+3125 = 3,905 | ~7,800 |
| `:4fa 8` | 8+64+512+4096 = 4,680 | ~9,400 |

Chain stages (a `:fa`, then `:ca` to deepen survivors, then another `:fa`) and the leaf base `L` carries
over. **The CLI's `:fa` is UNCAPPED** — the GUI caps pending fan at 96, the headless runner does not, so it
will generate every branch you ask for. You size the run.

## Runtime math — how to hit ~a day

Every node is one generation, so `wall_clock ≈ nodes × per_gen_seconds`. **Calibrate first**: have the user
run a small script and read the time — `gen-runs.sh` prints `seconds` and `nodes` per run, so
`per_gen ≈ seconds / nodes`. Then size to the target:

```
target_nodes ≈ desired_hours × 3600 / per_gen_seconds
```

As a rough prior, an M4 Max on an 8B-Q4 reasoning model at `--max 1536` lands somewhere around 10–25 s per
generation (reasoning + code; the KV prefix is reused across fan siblings, so prefill is cheap). At ~15 s,
~5,700 nodes ≈ 24 h — but **use the measured number**, not this guess.

## What makes a *good* large test (not just a big one)

- **Realistic, varied tasks** — parsers, allocators, ring buffers, state machines, small codecs, a
  bytecode VM, a query planner. Variety makes the branches explore real solution space, not noise.
- **Deep decomposition** — `:fill` on skeletons with **many `todo!()` holes** (8–20). Each hole is a
  KV-shared sub-task; multi-hole skeletons are the signature stress case. Put these skeletons in `.chat.md`
  seeds and `:open` them.
- **Wide exploration with carry-over** — `:fa` templates that use `{lastmsg}` so each variant builds on its
  own leaf rather than restating the prompt.
- **Interleave deepening** — `:fa` to branch, then `:ca` to extend promising lines, then `:fa` again.
- **Checkpoints** — periodic `:best` and `:export md` so a day-long run leaves inspectable artifacts
  partway through, not just at the end.
- **Reasoning-model hygiene** — run with `--max 1536` (or more) and `--system ''`; the runner strips
  `<think>…</think>` and extracts code on `:fill`.

## Deliverable

Produce, as files:

1. Several escalating `.logscope` scripts — e.g. `05-…` (a few hundred nodes) up to a final `NN-marathon`
   (the day-long one). Name them `NN-shortname.logscope`.
2. Any `.chat.md` seeds they `:open` (multi-hole skeletons especially).
3. For **each** script: a one-line note with the **predicted node count** (from the branching math) and the
   **estimated wall-clock** at the user's measured `per_gen`.

Keep every command inside the grammar table above. Validate the count math before quoting it.

## How the user runs them

```bash
# build once (lean, no GUI deps):
cargo build --release --bin logscope-cli --no-default-features --features wgpu-engine

# batch a numbered range into a subdir (mock for a dry shape-check, then the real model):
./gen-runs.sh range 5-9   model mock                          subdir ./runs/dryrun/
./gen-runs.sh range 5-9   maxtokens 1536  model ~/models/DeepSeek-R1-Distill-Llama-8B-Q4_K_M.gguf \
              system '' subdir ./runs/marathon/
```

`gen-runs.sh` writes `<name>.md` (the artifacts) and `<name>.log` (notes + final node count) per script and
prints a `script / nodes / seconds` table. `maxtokens` → `--max` (per-reply generation cap, not context
size). Tip: do a `model mock` pass first to confirm the branch *shape* and node counts off-GPU, then run for
real.
