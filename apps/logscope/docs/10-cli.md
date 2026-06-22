# 10 · The headless CLI and building a regression suite

`logscope-cli` runs a command-script through the same conversation-graph engine as the GUI, with no
window — so the flows we can't click through in a sandbox (continue-all depth, fuzzy-all fan-out, skeleton
fill, best-region selection) can be **scripted, run, and captured to a file**. You run the real ones on
the M2 and pipe the output back; the deterministic mock lets the orchestration be tested anywhere.

## Build

```bash
# lean CLI — no eframe/egui pulled in (fast, small):
cargo build --bin logscope-cli --no-default-features

# CLI wired to the real wgpu engine (for running on the M2):
cargo build --release --bin logscope-cli --no-default-features --features wgpu-engine
```

The GUI is unchanged: `cargo build` (the `gui` feature is on by default). The crate is now lib + two
bins; the CLI's whole module closure is egui-free, which is why `--no-default-features` drops the graphics
stack entirely.

## Reasoning models (R1-Distill, QwQ, …)

A first M4-Max run with `DeepSeek-R1-Distill-Llama-8B-Q4_K_M` surfaced two things worth knowing:

- **Budget.** Reasoning models think *before* they answer, so a small reply budget truncates them
  mid-thought and they never reach the code. Use `--max 1024` (or 2048) for these — 256 isn't enough to
  finish even the reasoning. The default is now 512; reasoning models still want more.
- **Extraction.** A reasoning reply is `[chain-of-thought] [maybe a code block]`, so `:fill` no longer
  splices the reply verbatim — it strips a `<think>…</think>` channel, takes the last fenced block (or the
  trailing `fn …{ body }`), and splices just the code. That's why a fill from R1 assembles real code rather
  than the model's "Okay, I need to…" prose.
- **System prompt.** R1-Distill is happiest with no terse-coding system prompt fighting its format; pass
  `--system ''` for a cleaner run.

```bash
logscope-cli --model ~/models/DeepSeek-R1-Distill-Llama-8B-Q4_K_M.gguf \
  --max 1536 --system '' examples/02-lru-fill.logscope > runs/02.md
```

Note also that `:best` here still uses the code-block proxy (any ```fence``` = pass), which over-counts:
the R1 run reported "4/4 verified" on code that wouldn't compile. That's the motivation for the
`cargo check` verdict oracle (see ch.09 §3) — it replaces the proxy with a real pass/fail.

## The compile oracle (`:check`) — real verdicts

Three real runs showed the same thing: `:best`'s code-block proxy (any ```fence``` = pass) is meaningless on
small-model output — it called a 1B's hallucinated `bash` blocks and an 8B's non-compiling Rust "verified."
`:check` replaces that with a compiler. It compiles every assistant node's code and stores Pass/Fail;
`:best` then ranks on real verdicts (and prints `[compiled]` vs `[code-block proxy]` so you know which).

```text
> write a function to add two numbers
:2fa 4 'another version: {lastmsg}'
:check          # compile each branch's code
:best           # now ranks by what actually builds
```

Default checker: `rustc --crate-type lib --edition 2021` (grades self-contained snippets; fragments and
cross-crate refs correctly fail). For real-crate grading — e.g. the batchwire `:fill` against the engine —
point it at the crate and tell it where to drop the code:

```bash
logscope-cli --model … --check 'cargo check -p unendlich-engine --features render' \
  --check-file crates/unendlich-engine/src/sim/service_registry_batch.rs  examples/05-batchwire-fill.logscope
```

On your two uploaded runs the oracle gets it right where the proxy didn't: the 8B's broken generic sort →
**FAIL**, the 1B's `cargo build` bash block → **FAIL**, the 8B's correct LRU `new` body → **PASS**.

## Timing the graph speedup

The KV-prefix-tree is what makes branching cheap (it reuses a shared prefix's cache instead of re-prefilling
each branch). To measure what it's worth, run a script twice and compare the `gen(s)` column — once normally,
once with the tree disabled:

```bash
./gen-runs.sh range 6-6 model ~/models/…gguf subdir ./runs/kv-on/            # speedups on
./gen-runs.sh range 6-6 model ~/models/…gguf nokv on subdir ./runs/kv-off/   # full prefill every turn
```

`nokv on` sets `LOGSCOPE_KV_OFF`, which forces a full prefill on every generation (no anchor reuse at all).
The runner times only the model calls (`gen(s)`, separate from `wall(s)`), so the difference is the prefix
tree's contribution — largest on branchy scripts (`:fa`/`:fill`) where many generations share a prefix. (On
the M2 a single warm resume already cut prefill ~6×; `nokv on` vs a `LOGSCOPE_KV_TREE=6` run shows the
fan-out case.)

## The script format (`.logscope`)

One step per line:

```text
# a comment (ignored)
> a user message                 send a user turn; the model replies   (or:  say a user message)
a bare line                      also a user message
:2ca                             a colon command — the SAME language as the GUI
:fa 3 'explore: {lastmsg}'
:best
:export md
```

Wired commands: `:ca [N]` (continue the active subtree's leaves, N rounds), `:fa [W] ['tmpl']` (fuzzy-all: W variants per
leaf, depth from the count, `{lastmsg}`/`{msgid}` filled per leaf), `:fill [path]` (decompose a `todo!()`
skeleton — from a file, else the last reply's code block — fan one KV-shared sub-task per hole, then
assemble), `:assemble`, `:quest [N]` (clarify-first: emit N questions as a fillable Q/A template, multiple-
choice where sibling branches suggest options), `:ans <path>` (fold a filled Q/A file into one focused
generation), `:best`, `:open <path>`, `:goto N`, `:export md|ascii|git`, `:qa`/`:stop` (cancel). Anything
else logs "skipped" so a script that uses a GUI-only command still runs.

### Clarify-first (`:quest` / `:ans`)

Front-load the ambiguity so the expensive generation is aimed. `:quest 4` asks the model (out of band — no
tree nodes) for the four decisions that would most change the implementation and prints a template:

```text
Q: Which data structure should back it?
   [a] Vec   [b] HashMap   [c] BTreeMap
A:
Q: What is the eviction policy when full?
A:
```

Multiple-choice options come from the question's own `(a / b / c)` or are harvested from sibling-branch
text; the rest are open. Fill the `A:` lines (by hand, or from another branch's findings), save the block to
a file, and `:ans answers.md` folds the decisions into a single focused turn — one targeted completion
instead of a fan of mis-aimed ones. In the GUI, `:quest` posts the questions as a reply and `:ans` reads the
filled block from the composer.

## Run

```bash
# deterministic mock (anywhere) — exercises the control flow:
logscope-cli --mock examples/02-lru-fill.logscope

# real model on the M2 — capture the artifacts:
logscope-cli --model ~/models/qwen2.5-coder-3b-q8_0.gguf examples/01-sort-generics.logscope > runs/01.md
```

Flags: `--model GGUF` | `--mock`, `--system TEXT`, `--max N` (reply token budget), `--final md|ascii|git`
(the whole-tree dump appended at the end).

**Output discipline:** requested artifacts (`:export`, `:best`, `:fill` assemblies) and the final tree go
to **stdout**; progress notes and the node count go to **stderr**. So `… > runs/01.md` captures only the
artifacts, and `2> runs/01.log` keeps the trace separately.

## Building a suite

The loop the CLI is built for:

1. Write (or have a model draft) a `.logscope` script per scenario — see `examples/`.
2. On the M2: run each against the real model, capturing stdout to `runs/<name>.<model>.md`.
3. Commit those captures. They're the regression baseline.
4. After an engine/sampler change, re-run and `diff` against the baseline. Divergences are exactly the
   behaviours to inspect — a sampler regression, a prompt-template change, a KV-reuse bug all surface here.

```bash
# capture a whole suite in one go:
for s in examples/*.logscope; do
  name=$(basename "$s" .logscope)
  logscope-cli --model "$MODEL" "$s" > "runs/$name.md" 2> "runs/$name.log"
done
```

Because the same script runs against the mock and the real model, the *shape* of every run (how the tree
branches, which commands fire) is verifiable off-GPU; only the generated text needs the M2.

## Honest scope

It's a test harness, not the app, so the prompt builder is a minimal ChatML/Llama-3/Plain template over
the lineage (no open-log injection), `:ca` appends a fresh assistant child per leaf rather than literally
resuming the same message's KV, and `:fill` assembles synchronously. The command *grammar*, the
tree-building, the fan-out counts, the fill decomposition, and the export are exact and tested; the reply
text and timings come from whichever backend you point it at.

## Model classes — a true multi-model pool

The runner holds a **pool** of models, not one. Pass `--model` more than once, naming each, and switch
between them mid-script with `:use <name>`:

```
logscope-cli \
  --model big=~/models/qwen2.5-coder-7b-instruct-q4_k_m.gguf \
  --model small=~/.local/share/logscope/llama-3.2-1b-instruct-q4_k_m.gguf \
  --max 1024 examples/05-multimodel.logscope
```

A bare `--model ~/x.gguf` is named `default`; `--model name=mock` makes a deterministic stub (handy for
dry-running the control flow off-GPU — `--model big=mock --model small=mock`). `:use big` / `:use small`
route every subsequent generation (`say`, `:fa`, `:ca`, `:fill`, `:quest`) to that model; an unknown name is
a no-op that leaves the active model unchanged and logs the available names.

This is what makes the thesis concrete: **assign each phase of the work to the model that fits it.** The
canonical pattern (examples/05) is scaffold-big / fill-small / grade-with-rustc:

```
:use big
> Write the struct + impl, leave each method body as todo!().
:use small
:fill                 # small model fills every hole — cheap, and they KV-share the skeleton prefix
:use big
> Review the assembled result for correctness.
:check                # the compile oracle — no model needed, just rustc
```

Each backend keeps its **own** KV anchors, so a `:use` switch is a full prefill on the newly-active model
(it hasn't seen the lineage — expected and correct). The prefix-tree reuse you measure in `06-kv-bench`
happens *within* a single model's run: once you're on `small`, its `:fill` sub-tasks all share the skeleton
prefix and reuse it. So switch models at phase boundaries, not inside a tight fan, and you pay the cross-model
prefill once per phase rather than per generation.

Memory note: every model in the pool is resident at once. Two small models (a 1B + a 7B, int8) sit
comfortably on a 64 GB M-series; size the pool to fit. Token-level multi-model — a draft model proposing and
a larger model verifying each token (speculative decoding) — is a deeper, engine-level change and isn't what
`:use` does; `:use` is command/phase-level routing.

## Merging a shortlist for a bigger model: `:mergectx`

After fanning out cheap candidates, you usually want one capable model to *pick* — not to re-run the
whole branchy thread, just to compare the replies side by side. `:mergectx` does exactly that:

```
:mergectx -1,3df,rro,a9j            # flatten these nodes into one context, then generate against it
:mergectx -1,-3,-5 'pick the best'  # with an explicit instruction
```

It resolves each id-ref, lays the referenced nodes out as a numbered **candidate list** (deliberately
*without* their branching ancestry — flat, so the picker compares replies, not threads), appends an
instruction (default: "pick the single best, name its id, say why"), and runs the **active** model
against that merged context. So the pattern is: `:use small` → `:fa N …` (explore) → `:use big` →
`:mergectx <the N leaves>` (judge). In the GUI, `:mergectx` stages the same merged text in the composer
so you can send it against whichever model you like.

### Addressing nodes by id

Every node has a short, stable id (a 6-char base36 hash, e.g. `a9j3k2`) shown in the GUI next to each
turn. Refs are resolved three ways:

- **id prefix** — any *unique* prefix works, git-style: `a9j`, or just `a9` if that's unambiguous.
  An ambiguous prefix resolves to nothing (the CLI logs it; type more characters).
- **`-K`** — the Kth node from the end. `-1` is the last node, `-3` the third-from-last.
- **a bare number** — that 1-based position in the tree.

The GUI offers inline completion: start typing an id after `:mergectx` or `:goto` and the matching ids
appear as clickable chips that fill in the rest.

### GUI command palette

In the GUI, typing `:` opens a zsh-style completion grid of every command whose name or alias extends
what you've typed (`:f` → `fuzz`, `fa`, `fuzzd`, `fill`, …), name-prefix matches ranked ahead of
alias-only ones. A hint line above the grid describes the top choice and its argument signature, and
keeps showing that description as you type the arguments (so `:mergectx ` still reads
"mergectx <ids> ['ask'] — flatten the referenced nodes into one context for a pick"). Clicking a chip
fills in `:<command> `, ready for its arguments. The same catalog backs both the grid and the hint and
lives in `vimmode::command_catalog`, so it stays in sync with the parser.

## Bird's-eye synthesis: `:strategy` / `:birdseye`

Where `:mergectx` *picks* a winner, `:strategy` *unifies and condenses*. It collapses a sprawling,
branchy exploration into one tight overview:

```
:strategy                         # condense the whole tree into a bird's-eye synthesis
:birdseye 'focus on the perf tradeoffs'
:strat -1,3df,a9j                 # synthesize just these branches
:bird -1,-3 'why these diverge'
```

Aliases: `:bird`, `:birdseye`, `:strat`, `:strategy`. With no ids it takes the entire conversation; with
ids (prefix / `-K` / position, same as `:mergectx`) it takes just those branches. An optional quoted
string steers the focus. It flattens the selected turns (each capped) into one transcript behind the
summarizer-worker prompt (`vimmode::STRATEGIST_PROMPT`), generates with the **active** model, and attaches
the digest at root — opening a fresh, smaller space rather than extending a branch. The worker is told to
produce a through-line, the distinct threads (merging duplicates across branches), what's settled, what's
open, and a single recommended next step — compressing hard and not manufacturing coherence. Pair it with
`:use big` after a wide `:fa` to get a strong model's strategic read of everything the small model explored.
In the GUI it stages the same digest in the composer to send against whichever model you pick.

## Mapping a chat into blocks: `:segment` / `:blocks`

Where `:strategy` condenses to one digest, `:segment` cuts the conversation into coherent **blocks** and
names the **seams** between them — so a long, messy linear chat becomes a richer graph (each block a node,
each seam a place to split or branch).

```
:segment                       # map the whole conversation into blocks + seams
:blocks 'where does the topic change'
:seg -1,-3,a9j                 # map just these turns
```

Aliases: `:segment`, `:blocks`, `:seg`. Same id/focus arguments as `:strategy`. It flattens the turns with
`[#n role · id]` markers (so the model can cite the spans it cuts) behind the segmenter-worker prompt
(`vimmode::SEGMENTER_PROMPT`), generates with the active model, and attaches the block-map at root. Each
block carries a title, the span, a relation to earlier blocks (new-topic / continues / refines /
feedback-on / correction-of / digression / loop), 3-6 keywords, a one-line gist, the boundary signal that
opened it, and a health flag — plus a closing "seams & graph notes" section calling out the strongest split
points and any stretches (e.g. a model looping) that should collapse to a single node. The relation tags
and keywords map straight onto graph edges and the convograph's overlap signal; collapsing a flagged loop
and branching at a "new-topic" seam are exactly the moves that make the graph richer than a flat spine.
