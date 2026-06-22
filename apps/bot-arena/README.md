# Bot Arena

A GTK4 + libadwaita desktop GUI for running a population of code-agents against a
git workspace: provision N isolated environments, give each worker a *purpose*
drawn from a corpus-grounded latent space, let each attempt the next change, score
them, keep a diverse elite, repopulate from survivors, repeat — with a coordinator
that learns which region of the swarm to deploy.

## Build status: VERIFIED

This compiles and runs. It was built and launched (under Xvfb) in a Linux sandbox:

```
cargo build --release   ->   Finished release [optimized] in ~5 min (first build)
```

The toolchain used was the Ubuntu 24.04 distro Rust (cargo/rustc 1.75) with GTK
4.14 and libadwaita 1.5. Because 1.75 is older than what gtk4-rs 0.9 needs, the
crates are pinned to the **0.8 generation** (see `Cargo.toml`), which builds on
1.75. On a newer toolchain (rustc >= 1.80) bump to gtk4 0.9 / libadwaita 0.7 and
delete the transitive pins. `Cargo.lock` is included for a reproducible build.

## What you get

Three pages in an Adwaita view-switcher:

- **Setup**
  - sliders: population, workers per container, keep-fraction (survivors), generations
  - backend selector (Simulation / Docker / Conda / Local), with a test command
    field used by the real backends
  - container image field + `docker load` image importer
  - editable tool list (allowed binaries)
  - workspace (git repo) picker
  - **Latent space**: corpus picker, behavior-niches slider, a *coordinator* switch,
    and a **latent-axis scrollbar** (the purpose-focus knob)
- **Arena** — one cell per bot, recolored live by lifecycle state, each showing
  fitness; hover a cell for its purpose, latent coordinate `z`, and fitness.
- **Logs** — a scrolling transcript: provisioning, per-generation best/mean,
  coordinator focus moves, culls.

## The latent layer (the "missing piece")

Workers are no longer undifferentiated. The engine grounds the swarm in a text
corpus and gives every worker a *purpose* coordinate `z` in [0,1]:

- **Corpus -> purposes.** The corpus is tokenized and split into `niches` bins,
  each tagged with a representative word. A worker's `z` maps to a niche, so each
  worker is literally *associated with text and purpose*. (The principled version
  drops in `vectors.bin` / `transitions.bin` from the v1.2/v1.3 pipeline as the
  real latent space — the transition clusters already are a vocabulary of
  purposes. This build uses a lightweight word-bucketing so the loop is
  self-contained.)
- **Quality-Diversity selection (MAP-Elites).** Instead of one global best winning,
  the top fraction *within each niche* survives, so the swarm spreads across
  purposes rather than collapsing to one.
- **The latent-axis knob.** The scrollbar drives a shared atomic the engine reads
  each generation — the coordinator's current *focus*. Workers near the focus get
  a deployment bonus (the coordinator is using that purpose-region).
- **A coordinator that learns the axis.** With the switch on, each generation the
  coordinator moves its focus toward the niche with the best reward (a hill-climb
  over the latent axis). Validated separately: focus converges onto the best niche
  in ~13 generations. That's a manager learning to use the swarm.

## Build

### Dependencies (Debian/Ubuntu — the exact set that worked here)

```bash
sudo apt-get install -y \
  cargo rustc cmake pkg-config build-essential \
  libgtk-4-dev libadwaita-1-dev
```

Fedora: `sudo dnf install gtk4-devel libadwaita-devel gcc pkgconf cargo`
macOS: `brew install gtk4 libadwaita pkg-config rustup-init && rustup-init`

### Compile and run

```bash
cargo run --release
```

Docker backend needs the `docker` CLI + daemon; Conda needs `conda` on `PATH`.

## Using it

1. On **Setup**, set the sliders (population 100, keep 30% by default). Pick a
   corpus to ground purposes (optional — without one, purposes are generic names).
   Set the niches count. Decide manual vs. coordinator: leave the switch off and
   drive the **latent-axis scrollbar** yourself, or turn it on and let the
   coordinator learn the focus. Pick a backend, tools, workspace.
2. Press **Start**. The Arena animates; with the Simulation backend you'll see the
   population go green over generations while staying spread across niches.
3. Watch **Logs** for coordinator focus moves and per-generation stats. Hover a
   bot to see its purpose and latent coordinate.

## Real evaluation (no longer a stub)

`evaluate_real` in `src/engine.rs` now does the actual loop. For the real backends
(Local / Docker / Conda) each worker:

1. gets an **isolated copy** of the workspace (`copy_dir_recursive`, skipping
   `.git`/`__pycache__`/`node_modules`/`target`/`.venv`);
2. **attempts an edit** — a purpose-typed mutation where the worker's latent
   coordinate `z` selects the *kind* of edit (flip an operator / comment a line /
   duplicate / delete / no-op baseline);
3. **runs the test command** (`pytest -q` by default) — locally, via `conda run`,
   or copied into the container and run with `docker exec`;
4. is scored by **pass-fraction** (parsed from the pytest summary, with an
   exit-code fallback for non-pytest runners).

Fitness for real backends *is* that measured pass-fraction — the coordinator's
focus only steers which purposes survive, never the metric, so the reward can't be
gamed. The coordinator therefore learns *which kind of edit* tends to pass tests.

**Verified:** `cargo test` builds tiny pytest repos and checks that a clean repo
scores 1.0, a broken one scores less, the parser is correct, and the full
population loop runs real evaluations end to end (4 tests, all passing). Run them:

```bash
pip install pytest      # the tests shell out to pytest
cargo test --release
```

### Making it intelligent — the llama.cpp agent (wired, feature-gated)

The mutation in step 2 is a deliberately dumb baseline. The real agent is already
wired behind the `llamacpp` Cargo feature: `worker_attempt` calls `llm_edit`, which
hands the active local model the failing test output plus one source file and asks
for a rewrite, then writes it back and re-runs the tests. If there's no model or
the edit fails, it falls back to the mutation baseline — so the default build still
runs.

- `src/llama_backend.rs` is the in-process GGUF inference backend (Vulkan), adapted
  from the uploaded module: `load_model`, `generate`, an active-model registry.
- **Default build (feature off):** no llama dependency, no Vulkan needed; builds on
  rustc 1.75 and all tests pass. This is what's verified here.
- **Enabling it:** add `llama-cpp-2 = { version = "0.1", features = ["vulkan"] }`
  to `[dependencies]`, set `llamacpp = ["dep:llama-cpp-2"]`, and
  `cargo build --release --features llamacpp` on a machine with clang + a Vulkan
  SDK. Verified that with the feature on, the *only* unmet requirement is that
  crate — the wiring itself compiles. The FFI body is DRAFT until built on hardware
  (same caveat as the original module).
- Load a model and publish it with `llama_backend::set_active_model(...)`; every
  worker then edits with it. `z` (purpose) is the natural place to vary the prompt
  per worker (e.g. "minimal fix" vs "refactor") — a one-line change in `llm_edit`.

## Backends

| Backend     | Status            | Notes |
|-------------|-------------------|-------|
| Simulation  | works, no deps    | Synthetic genome landscape; animates the loop instantly. |
| Local       | **real, verified**| Runs the test command on the host in per-worker worktree copies. |
| Conda       | real              | Same, wrapped in `conda run -p <env>`. Needs `conda`. |
| Docker      | real              | Worktree copied in with `docker cp`, tested via `docker exec`. Needs a daemon. |

## Data registry (organized corpora + binaries)

Everything shared lives under one root (`$XDG_DATA_HOME/codebot-arena`, override with
`CODEBOT_ARENA_HOME`):

```
<root>/bin/                       pipeline binaries  (built by `make -C pipeline`)
<root>/corpora/<epoch>_<name>/
    corpus/                       the text the artifacts came from
    artifacts/                    tokenizer.bin, vectors.bin, transitions.bin, gpt.bin, ...
```

The C pipeline now ships in `pipeline/` — `make -C pipeline` compiles all ten
binaries straight into `<root>/bin`, so Corpus Studio finds them automatically
(no more "point at v1.5-complete/bin", which is why a Run could look dead — if the
binaries dir is unset the run quietly bails). Both GUIs now show a **Corpus
(registry)** dropdown listing the timestamped corpora; pick one instead of
browsing. Studio defaults its binaries/output dirs to the registry, and a missing
corpus now jumps to the Pipeline page with a loud message rather than a silent
status line.

**Using a tokenizer/vectors you already built:** drop them into
`<root>/corpora/<epoch>_<name>/artifacts/` (with the source text in `corpus/`) and
they appear in the dropdown. (Note: Bot Arena currently derives worker purposes
from the corpus *text*, not yet from `transitions.bin` — consuming the real
embeddings is the next wiring step.)

## A note on the test command (and mixed repos)

Reward = test pass-fraction. Leave the **Test command** at `auto` and the scorer
**detects** the build/test systems in the worktree and runs each, summing results:

| marker file        | system | command run                                   | parsed from            |
|--------------------|--------|-----------------------------------------------|------------------------|
| `Cargo.toml`       | cargo  | `cargo test --workspace`                      | `N passed; M failed`   |
| `CMakeLists.txt`   | ctest  | configure → build → `ctest`                   | `X% tests passed … out of Y` |
| `meson.build`      | meson  | `meson setup` → `meson test`                  | `Ok: / Fail: / Timeout:` |
| `pyproject`/tests  | pytest | `python3 -m pytest -q`                         | `N passed`             |
| `package.json`     | npm    | `npm test`                                    | jest `Tests:` line     |
| `go.mod`           | go     | `go test ./...`                               | exit code / `--- FAIL` |

**Mixed repo (CMakeLists + a Cargo workspace + subrepos):** detection walks the
tree (depth ≤ 4, skipping `target/`, `build/`, `node_modules/`, …) and keeps the
**shallowest root per system per branch**, with cargo handled by *workspace
semantics* — a member crate is dropped only if an ancestor manifest declares
`[workspace]` (a nested crate that declares its own `[workspace]` stays
independent). It then runs every kept root and the score is

```
reward = (sum of passed across all systems) / (sum of all tests)
```

So a repo with a root CMake project (2/3 ctests) and a Cargo workspace (2/3 tests)
scores `4/6 = 0.67`. A build failure isn't free — it contributes `0 / ≥1`, dragging
the score down. The arena logs the detected systems at run start so the reward
signal is transparent. *(Verified end-to-end: a live cargo+ctest repo scores exactly
4/6 in the test suite.)*

If every bot scores 0.000, the command is wrong for the project — e.g. `pytest` in
a Rust repo returns "no tests ran" (exit 5). `auto` avoids this; the parser also
handles cargo's multi-crate summaries (it **sums** the `test result:` lines).

## Build caching (faster iterations)

Toggle **Build cache** (on by default). The scorer injects, for every bot:

- `CARGO_TARGET_DIR` → one shared dir under `<registry>/cache`, so the dependency
  graph compiles **once** for the whole swarm and each bot only rebuilds its own
  changed crate. Measured here: a cold build of a regex-dependent crate took
  **18.9 s**; the next bot sharing the target took **1.2 s** (~16×).
- `CC`/`CXX` → `ccache cc`/`ccache c++` and a shared `CCACHE_DIR`, plus
  `-DCMAKE_<LANG>_COMPILER_LAUNCHER=ccache` for CMake — identical C/C++ compiles
  across bots become cache hits.
- `RUSTC_WRAPPER=sccache` **iff** `sccache` is on PATH (it isn't required; the
  shared target dir already gives most of the Rust win).

## Invalid commands: penalize the bot, or flag the harness?

Both, but they're different failures and the engine now treats them differently:

- **The bot's edit broke the build or failed tests** → score `0` (or partial). That
  *is* the penalty — selection culls it. No negative reward is needed; `0` already
  sorts it last. A non-compiling edit naturally scores worse than one that compiles
  and fails some tests, so the gradient is already there.
- **The command itself can't run** (missing binary, `cargo`/`cmake` not on PATH,
  exit 127 — exactly your `failed to launch: No such file or directory`) → that's a
  **harness** error, *not* the bot's fault. Scoring every bot 0 in that case looks
  like the algorithm failing when really the setup is broken. So at run start the
  engine preflights the tools each detected system needs: it **warns** and skips a
  system whose tool is missing, and **aborts with a clear message** if nothing is
  runnable — rather than grinding out generations of zeros. In `auto` mode a system
  whose tool is absent is simply not counted (so a missing `meson` doesn't drag a
  passing `cargo` score down).

(Your specific error was the pipeline binaries not being built — Corpus Studio now
refuses to run and tells you to `make -C pipeline` instead of failing each stage.)

## Provisioning & locking (why iterations are slow, and what actually helps)

Each bot is evaluated in its own throwaway worktree. Two levers:

- **Provisioning.** Each bot gets a throwaway worktree, built by the cheapest
  viable strategy (Fast clone on):
  1. **git worktree** — committed HEAD, shares the object store (git repos);
  2. **rsync `--link-dest`** — hardlinks unchanged files against the original, so
     near-zero IO when the worktree is on the same filesystem (verified: 50 files
     shared inodes, nothing copied);
  3. **`cp --reflink=auto`** — copy-on-write on btrfs/xfs (this is the portable
     "btrfs snapshot" win — no subvolume setup required);
  4. **recursive copy** — universal fallback.
  Strategies 2–4 skip VCS/build noise. Hardlink/CoW provisioning is only safe
  because **edits are now atomic** (write-temp-then-rename), so the shared inode is
  never modified in place — verified by test. (btrfs *subvolume* snapshots are even
  cheaper but require the workspace to be a subvolume with the right permissions;
  `--reflink` gets you btrfs CoW without that ceremony. For the rsync/reflink IO win
  the worktree must be on the same filesystem as the workspace.)
- **Build cache + locking.** This is the real iteration-speed lever, and the locking
  worry is largely a non-issue *as currently run*: bots are evaluated **sequentially**,
  so the shared `CARGO_TARGET_DIR` lock is never contended — one bot builds at a time
  and reuses the previous bot's compiled dependencies (the 18.9s→1.2s win). You do
  **not** want to "undo" the shared cache to avoid locking; that would make every
  build cold again. The lock only bites if you later evaluate bots **in parallel** —
  and the fix then is *not* dropping the cache but giving each concurrent bot its own
  target dir and leaning on `sccache` (built for concurrent access, caches per
  compilation with no global build lock) for cross-bot reuse. Until then: keep the
  cache on, keep eval sequential, and the lock costs nothing.

If a real run still feels slow, the usual culprits are: the cache toggle off; a wrong
test command (cold/again-and-again full builds, or `pytest` in a Rust repo); or an
edit low in the dependency graph forcing wide recompiles (inherent — `CARGO_INCREMENTAL=1`
is now set to soften it).

## Search vs. learning (the honest state)

**Do they bootstrap structure from noise → tensors → characters → commands?** No —
that's a description of *training a model from scratch*, which is what the corpus
pipeline's `tiny_gpt` does (random weights → gradient descent on the corpus →
structured token distributions). The **arena** does something different and much
simpler:

- *Default (no `llamacpp`):* each worker runs `mutate_source` — a typed edit of
  *existing* source (flip an operator, add a comment, duplicate/delete a line, or
  no-op), chosen by its purpose coordinate. No tensors, no character generation —
  structured random mutation plus selection. A genetic algorithm.
- *With `llamacpp` + a model:* the worker samples a **pretrained** model to rewrite
  a file. The structure was learned during pretraining; the arena only *samples* it
  and keeps what passes tests. The model's weights never change here.

Either way the loop is **search**, not learning: offspring inherit a purpose
coordinate, nothing in any model updates, so the swarm doesn't get better at coding
over time — it explores a fixed policy's outputs. Bootstrapping with tool docs is
*context*, not learning.

To actually learn, winners must update something persistent — cheapest to heaviest:
(1) a retrieved patch library keyed by latent purpose; (2) heritable prompts;
(3) **expert iteration** — log winning demonstrations and fine-tune the policy (LoRA
on the llama backend, or train `tiny_gpt`) on them, reload, repeat. Only (3)
compounds, and it's the natural next subsystem.

## Invalid commands: penalize the bot, or flag the harness?

A subtle point worth restating in light of "what the workers do": the workers don't
*emit commands* today — they edit a file, and a fixed harness command tests it. So
there's nothing for a worker to get "invalid." The two failure modes are:

- **The edit broke the build / failed tests** → reward `0` (or partial). That *is*
  the penalty; selection culls it. No negative reward needed.
- **The harness command can't run** (missing tool, exit 127) → not the bot's fault;
  the engine warns/aborts rather than scoring everyone 0 (see below).

If you later move to an *agentic* worker that emits shell actions, then invalid
actions become real, and the natural penalty is exactly the same: the action fails,
tests don't improve, reward stays low → selected against. An explicit per-failed-
action penalty is optional and only changes how fast the search avoids flailing.

## Score memoization, resource limits, crossover

- **Memoization.** Many bots converge on identical repo states (no-op edits, the
  same fix found twice). Each evaluation hashes the edited worktree (path + bytes,
  noise/large files skipped) and reuses a previously computed `(passed, total)`
  instead of re-running the test command. The cache persists to
  `<registry>/cache/scores.tsv`, so identical states are free across runs too.
  Toggle: **Reuse scores for repeated repo states**.
- **Resource limits.** Every command is wrapped: `ulimit -f` caps any single output
  file (~100 MB under bash) and a `timeout -k 5 <N>` kills runaways (default 30 s,
  SIGTERM then SIGKILL). Captured output is bounded to 256 KB. Verified: `sleep 30`
  dies at the timeout; a 200 MB write is killed by SIGXFSZ.
- **Crossover.** When repopulating culled slots, with crossover on the engine
  sometimes blends *two* survivors (genome averaged, purpose from one parent) rather
  than mutating one — recombination explores faster than mutation alone. Toggle:
  **Recombine survivors (hybrids)**.

## Toward a command-genome (and why not backprop)

A tempting design: each bot emits a **command string** (program + args + pipes),
and the population evolves these — mix-and-match arg subsets, splice fragments,
seed from `--help`/man and from filepaths parsed out of commands, with a mandatory
terminate/commit token the bot must learn to use. That's **genetic programming over
command lines**, and it's a real learning mechanism (the population improves via
recombination + selection). This repo now ships the *foundation*: `weighted_vocab`
(frequency-weighted tokens from help/man text) and `extract_filepaths` (path-like
tokens from a command), both tested. The GP loop itself (grammar, terminate token,
execute-and-rank) is the next subsystem.

One clarification on "do they learn by backprop against their toolset / a
transformer per command": you **can't backprop through running a test suite** — the
reward (tests pass/fail) is a black box, not a differentiable function of the
command. So gradient descent doesn't directly apply. The options that do:

- **Evolution / GP** (what the command-genome is): no gradients; crossover + mutation
  + selection. Sample-inefficient but simple and already half-built here.
- **RL (policy gradient / expert iteration):** treat the model as a policy, sample
  actions, and update it toward high-reward samples. Expert iteration (keep winners,
  fine-tune on them) is the practical version and the one that compounds.
- A **per-command transformer** would be trained by one of the above, not by
  backprop-through-tests. It's heavier than the evolutionary command-genome, so the
  pragmatic path is: evolve command-genomes now; later add a learned policy (the
  llama backend or a small trained model) that *proposes* fragments, improved by
  expert iteration on the winners the arena already produces.

## Git-derived signals, diff metrics, and test generation (design + foundations)

This is the most fertile area for *scoring* and *learning fuel*. What's built as
tested foundations, and how each idea pans out:

**Built now (tested, not yet wired into reward):**
- `merge_versions(a, b)` — **recombination of two versions** (e.g. a file at commit
  N-1 and N+1): shared lines (the LCS) are kept as anchors so the child stays
  structurally sensible — "rebased on lines," not a random character interleave — and
  each divergent hunk is taken from one parent at ~50/50. The child is then a
  candidate to *score* (better/worse than either parent). That's exactly the
  merge-as-genetic-crossover idea: blend N±1, anchor on common lines for coherence,
  let the harness say which blend is better. (A true 3-way merge against the real
  merge-base is the natural refinement.)
- `funcscan <file>` — lists functions and flags those with **no callers** as test
  candidates. Concretely answers "codebases have more testable things than tests":
  it points at the untested surface so a generator (or you) can add tests there.
- `diff_metrics(before, after)` — "concise vs creative": measures lines changed and
  *novel identifiers introduced*, scoring `novel / (1 + lines_changed)` so a small
  change that still introduces a new idea beats a sprawling one. **Caution:** as a
  *reward* this is gameable (gibberish tokens inflate novelty), so it belongs as a
  tie-breaker / diversity signal, never as the primary metric — test pass-fraction
  stays primary.
- git helpers — `git_recent_commits`, `git_commit_diff`, `git_patch_applies`. These
  underpin everything below.

**How the git ideas pan out:**
- **git patch → coherence (strong).** `git apply --check` is a clean, cheap, binary
  coherence signal: does this change integrate without conflict? Built
  (`git_patch_applies`). The natural reward for a patch-emitting worker.
- **git progression (strong).** Commit `N`'s diff vs `N-1` is a self-labeled "good
  change." Sample commits → `(state, diff)` demonstrations for expert iteration. The
  teardown/reverse you liked is the same primitive run backwards: *revert* a commit to
  inject a known bug, and the commit *is* the labeled fix — instant repair curriculum.
- **git rebase → adversarial (medium).** Replaying a change onto a shifted base and
  seeing whether it survives is a robustness probe, but rebase conflicts are noisy and
  setup-heavy; more useful as occasional stress than per-bot scoring.
- **git blame → coherence (weak/fuzzy).** Blame attributes lines to commits; turning
  that into a numeric "coherence" score is hand-wavy. Better used as *context* for a
  classifier (below) than as a metric.
- **git tags / branches (structural).** Tags = known-good checkpoints to evaluate
  against or target; branches = parallel lineages (a natural home for island-model
  evolution — separate populations that occasionally exchange migrants).

**Classifier audits (the creative/stochastic judge).** Using a model to *read the
change + its git context and rate it* is a learned reward. It adds the randomness and
"taste" you're after, but it's only as trustworthy as the judge and is itself
gameable — so it works best blended with the hard signal (tests pass) rather than
replacing it: hard reward gates, soft reward ranks within the gate.

The throughline: keep the **objective** signal (tests pass / patch applies) as the
gate, and use the **subjective** ones (concise-creative, classifier taste, blame) only
to rank and diversify inside it. That avoids reward-hacking while still rewarding
creative, coherent, concise changes.

## FSM bot type (`src/fsm.rs` + `fsmgen`)

An evolvable **weighted finite-state machine** as a generative bot genome — the
"super cool to see" structure. A genome is `N` states (slider cap), each emitting a
random character, a pipe `|`, a space, or a command token, wired by up to `M`
weighted out-edges per state (slider cap). "Running" it is a weighted random walk
that emits a string — the bot's generated command/edit. The module provides:

- `Fsm::random(n_states, max_conn, rng)` — build a capped random machine.
- `Fsm::run(max_steps, rng)` — weighted walk → emitted command.
- `Fsm::mutate(rng)` — reweight / add / remove an edge, or change a state's emission
  (preserving the ≤`max_conn` invariant) — the evolution hook.
- `Fsm::to_svg()` — render the state graph to a standalone SVG (node colour by kind:
  char/pipe/space/token; edge thickness by weight; dashed ring on the start state).
  This is the heroes-gallery **save-to-SVG**.

`fsmgen --states N --max-conn M --steps K --seed S --svg out.svg` exercises all of it
from the CLI (prints the emitted command and writes the SVG). A sample is in the zip.

**Heroes gallery (next GUI step):** a Setup/Arena tab listing the top scorers, each
drawn from `to_svg()`, with per-hero SVG download and "export command" (the `run()`
output). The rendering + export are done here as pure functions; what remains is
making the FSM the live bot genome (so machines evolve and get test-scored) and the
GTK gallery view — the next focused build.

## Scale & iteration speed

The RNG and per-generation pacing were genuine bottlenecks; addressed:

- **Seedable fast PRNG.** Hot paths used `thread_rng()` — a reseeding CSPRNG, far more
  than a GA needs and not reproducible. Switched to `SmallRng` (xoshiro), seeded
  per-(run, generation, thread) from a master **seed** (set it in the GUI for a
  reproducible run; blank = random, and the chosen seed is logged so a good run can be
  replayed). Cheaper per draw and deterministic.
- **Turbo / adaptive pacing.** Three `sleep(110ms)` calls per generation (a fixed
  ~0.33s/gen floor) plus a hidden `8ms × n_containers` provisioning delay (≈24s for
  3000 bots!) were capping throughput regardless of compute. The provisioning delay is
  gone; the generation sleeps are skipped under **Turbo mode** or automatically for
  populations > 500. Measured: **40 generations × 300 bots in ~25ms (~1600 gens/sec)**,
  versus the old ≥13s.
- **Less UI flooding.** The Sim sent two per-bot `Bot` messages (each allocating a
  purpose string) plus an `Activity` message every bot every generation; now one `Bot`
  message, and `Activity` is sampled (off above 200 bots).

## Batch processing (learning rate)

**Batch size** slider = best-of-k attempts per bot, annealing linearly to 1 by the
final generation (`batch_size(gen, …)`): broad sampling early raises the effective
learning rate, then it narrows to a single decisive attempt late. Applies to Sim and
real backends. Measured: by generation 3, batch=20 reached 0.936 vs 0.910 for batch=1.

## FSM bot type (`src/fsm.rs` + `fsmgen`)

An evolvable **weighted finite-state machine** as a generative genome: `N` states
(char / pipe / space / token), up to `M` weighted out-edges each; running it emits a
string. Now also: `random_seeded` (reproducible), `crossover` (recombine two
machines), `similarity` (LCS-based fitness), and **`evolve_toward(target, …)`** — an
elitist GA that makes the FSM actually *learn* (e.g. evolving toward `"hello"` →
`"headl xll ofl"`). `Fsm::to_svg()` exports the structure; `fsmgen --evolve hello
--svg hero.svg` evolves a hero and saves it. (The GUI heroes gallery — a top-scorer
grid of these SVGs — is the next GUI step; the genome, evolution, scoring, and export
are all in place.)

## Git scoring & diversification

- `diff_similarity(d1, d2)` — Jaccard over two diffs' changed-line sets (0 disjoint, 1
  identical). The **git scoring** basis: reward a candidate for resembling a real
  beneficial commit's change (use as a tie-breaker/diversity signal alongside the hard
  test gate, never as the sole reward — it's gameable).
- `diverse_commits(repo, k)` — **git diversification**: greedy max-min selection of
  `k` commits whose diffs are mutually most dissimilar, to assemble a varied
  curriculum / target set so the population doesn't overfit one kind of change.
- (Plus the earlier `git_recent_commits`, `git_commit_diff`, `git_patch_applies`,
  `merge_versions`.)

## Bundled agent tools

`cargo build --release` builds eight dependency-free helpers a code agent can call,
all seeded into the GUI tool list by default:

- **`bracecheck <file>`** — `()[]{}` balance, skipping strings/comments.
- **`astsketch <file>`** — definition outline indented by brace depth.
- **`funcscan <file>`** — functions + a "no callers / test candidate" flag.
- **`treelite [dir] [-L n]`** — a tiny `tree` skipping VCS/build noise.
- **`loc [dir]`** — lines of code/comment/blank by extension (tokei-lite).
- **`todos [dir]`** — scan for TODO/FIXME/HACK/XXX/BUG markers.
- **`jsoncheck [--check] <file>`** — validate + pretty-print JSON (no deps).
- **`findtext <pattern> [dir]`** — a tiny recursive grep.
- **`lexscan <file>`** — FSM lexer: token-class counts + unterminated string/comment flags.

(Plus coreutils `cat`/`head`/`tail` and, if installed, `rg`/`tokei`/`tldr`.)

## Edit operators (the toggleable "edit commands")

A worker's mutation is one of fifteen operators, each a SwitchRow in the **Edit
operators** group on the Setup page (all on by default — toggle to shape the search):
`flip-op`, `flip-bool`, `tweak-num`, `math-op` (wrap a number in arithmetic),
`string-op` (mutate a string literal), `comment-out`, `uncomment`, `dup-line`,
`delete-line`, `swap-lines`, `reorder` (reverse a block — reordering), `wrap-loop`
(wrap a line in a `for` loop — loop building), `splice-line` (intra-file
recombination), `insert-rand` (insert file-flavored random text), `no-op`.

The **Edit aggression** slider (0 = timid → 1 = aggressive) scales how *large* each
edit is: bigger reordered blocks, larger arithmetic multipliers, more loop
repetitions, longer `insert-rand` lines. `insert-rand` generates its characters with
`markov_chars` — an order-1 Markov chain over the file's own characters, a degenerate
one-step "RNN" whose output is gibberish but flavored like the file (its identifiers,
operators, spacing) rather than uniform noise. The purpose coordinate `z` selects
among the *enabled* operators, and the choice shows in the activity feed.

## Shelter mode (command safety)

The old "Sandbox (bubblewrap)" switch is now **Shelter mode**, doing more than
network/write confinement:

- **Denied binaries can never run.** A denylist (`rm`, `dd`, `mkfs`, `shred`, `wget`,
  `curl`, `ssh`, `sudo`, `mount`, `systemctl`, …) is enforced two ways: outside
  shelter, a *deny-shadow* PATH dir of refusing stub scripts is prepended so a bare
  `rm`/`curl` hits a stub that exits non-zero; in shelter, those binaries are never
  placed on PATH at all. (Verified: `rm` → "not found" in shelter, → "blocked" stub
  otherwise, while allowed tools like `grep` still run.)
- **Allowlist in shelter.** PATH is set to *only* an allow dir of symlinks to the
  tool list ∪ an essential toolchain set (`sh`, `rustc`, `cc`, `ld`, `make`,
  `python3`, coreutils, …) minus the denylist — so any binary not on the list is
  unresolved, i.e. rejected. The allow/deny dirs live under the registry cache (bound
  into the sandbox; `/tmp` is tmpfs-hidden so they can't live there).

The Setup tool list is the include/allow list feeding this. (A richer include/exclude
UI with move-between-lists and a system-binary search picker is the next GUI step; the
safety logic already works off the current list.)

## Sandboxing (the `cd`-escape guard) & parallel eval

- **Sandbox.** Toggle **Sandbox commands (bubblewrap)** and every local command runs
  under `bwrap`: network off, the whole filesystem read-only, writes allowed only in
  the worktree + build cache, and `/tmp` replaced by an empty tmpfs. The right way to
  "guard `cd` out of the working dir" is *confinement*, not parsing the command —
  `cd /home` or `cd ..` lands somewhere read-only with sibling worktrees invisible,
  so nothing outside the worktree can be modified. Verified: in-worktree writes
  succeed; `/home` writes, sibling reads, and network all blocked. If `bwrap` is
  missing, the engine warns and runs unconfined rather than failing.
- **Parallel workers (the 12× hypothesis).** Set **Parallel workers** > 1 and the
  real-backend eval fans out across threads, each thread a "slot" with its **own**
  `CARGO_TARGET_DIR` (`cache/cargo-target-<slot>`), so cargo's target lock is never
  contended. Honest ceiling: N workers approach ~N× throughput *once warm*, but each
  slot's first build is cold (no cross-slot dep reuse) and you pay N× the disk for N
  target dirs; `ccache` is shared (concurrency-safe) so C/C++ objects still cross
  slots, and `sccache` (if installed) would share Rust artifacts too. So 12 slots is
  a real speedup, just not a clean 12× — measure on your machine.

## Learning data: git progression & adversarial tests (design)

Two ideas about *where the learning signal comes from*:

- **Git progression batches.** The repo's own history is a free, self-labeled dataset
  of improvements: provision a worktree at a random commit `N` and the *next* commit's
  diff is a demonstration of "what a good change looks like." `git worktree add <dir>
  <sha>` makes pulling up random commits cheap (same primitive Fast clone already
  uses), so batches of `(state@N → change@N+1)` become training data for expert
  iteration. This is the most promising near-term learning fuel and pairs directly
  with the worktree provisioning already built.
- **Adversarial / generative tests.** A pure "destroyer" adversary is weak: breaking
  code is trivial and asymmetric — knowing *how to break* doesn't invert into *how to
  fix*, so it's mostly more search. The productive version is **curriculum
  generation**: an adversary that *injects bugs* (mutations that break specific tests)
  hands the fixer a stream of `(broken → known-good)` repair tasks, and an adversary
  that *writes new failing tests* (property/fuzz-style) pushes the fixer to
  generalize. That's genuinely more than search — it manufactures training tasks —
  and it's the synthetic counterpart to git-progression when history is thin.

A note on **nushell**: its structured pipelines (`ls | each { |r| cd $r.name; make }`)
are appealing for a command-genome, but it's a heavy dependency and doesn't change the
safety story — the `cd`-escape concern is handled by the sandbox above regardless of
shell. It's a reasonable optional command backend later, not a core dependency.

## Recurrent bots & multiple commands (the agentic fork)

Two related ideas — *should some bots be recurrent (iterate on previous state,
depth-bounded)* and *can a bot use multiple commands* — point at the same next
subsystem: a **bounded iterative worker**. Today a bot does one edit, then one
scoring pass. The agentic version loops up to `depth` steps (cap 100): each step
conditions on the previous result (the last test output / current worktree),
proposes the next action, applies it, re-scores, and stops on success or a
terminate/commit token. That *is* "multiple commands," and the "iterate on previous
state" is exactly a recurrent structure — the carried state is the worktree plus the
last result.

Note the distinction: a literal **RNN/transformer policy** is the *learned* version
of this loop, but you can't train it by backprop-through-tests (the reward is a
black box), so it'd be trained by evolution or expert iteration. The *structure*
(loop, carry state, cap depth) needs no neural net and is the cheap, honest first
build. Hill-climbing within a lifetime (keep a step only if it improves the score,
else revert) is the natural policy; memoization makes repeated states free.

The action a step proposes follows the pattern you described — **random chars →
string**, **random substrings → string**, or **LLM → string** — composed from the
`weighted_vocab`/`extract_filepaths` foundation already in the tree. That's the
command-genome; wiring the depth-bounded loop + terminate token + the three
proposal operators is the focused subsystem to build next.

## Live worker activity, the bundled converter, and threading

- **Activity panel.** The Arena page shows a live feed of what each worker is doing
  — its edit kind and the command it will run (e.g. `edit:flip-op → cargo + ctest`)
  — refreshed on a 3-second heartbeat (data updates continuously; the panel renders
  every 3s so it never thrashes).
- **Bundled HTML→MD converter.** `corpus_md` is a dependency-free, multithreaded
  Rust rewrite of `create-md-from-web.py` (strips `<script>/<style>`, decodes
  entities, maps headings/lists, cleans blank lines). Corpus Studio uses it by
  default (toggle **Use built-in HTML→MD converter**), so the pipeline needs no
  python or `html2text`. 200 files convert in ~7 ms across 8 threads.
- **Threading.** Sim evaluation now fans the population across threads
  (`available_parallelism`, capped at 8) — it's embarrassingly parallel (no
  filesystem, no build lock). Real-backend evaluation stays sequential on purpose:
  the shared `CARGO_TARGET_DIR` lock makes concurrent builds serialize, so parallel
  real eval would need per-bot target dirs + `sccache` (the locking trade-off
  described above), not just more threads.

## Files

```
Cargo.toml / Cargo.lock   pinned, reproducible build
src/main.rs               GUI: window, pages, sliders, latent controls, grid, logs
src/engine.rs             Config, messages, latent population loop, coordinator, backends
```

## Temp files & cleanup

Everything ephemeral is namespaced by process and swept automatically:

- **Bot Arena worktrees** live under `tmp/bot-arena-work-<pid>/run-<nanos>/bot-<id>`.
  Each per-bot copy is deleted **immediately after it's scored**; the run dir (and
  the now-empty parent) are removed in `teardown`; and `cleanup_workspace_temp` runs
  on app shutdown, so closing the window mid-run still leaves nothing behind. The
  `run-<nanos>` segment keeps concurrent runs (or a second window) from colliding.
- **Conda envs** created by provisioning are removed in `teardown` (`conda env
  remove` plus a directory sweep). **Docker containers** are `docker rm -f`'d, and
  the per-bot `/work-bot` copy is recreated each eval.
- **Corpus Studio staging** uses a single `tmp/corpus-studio/staged-<pid>` dir that
  is cleared on each "Stage to temp" and removed on shutdown — it never accumulates.
  The **output dir** (default `tmp/corpus-studio/out`) is intentionally *not*
  deleted: it holds your pipeline artifacts (tokenizer.bin, vectors.bin, …).

Verified by `cargo test` (`worktree_is_removed_after_eval`) and by checking that no
`bot-arena-work-*` dir survives a full run.

## Architecture notes

- Engine runs on a worker thread, never touches GTK; streams `EngineMsg` to the
  GUI over an `async-channel`, drained with `glib::spawn_future_local`.
- The latent-axis knob is an `Arc<AtomicU32>` (an f32 in bits) so the scrollbar can
  steer the running engine without extra plumbing.
- `workers_per_container` controls packing: containers = ceil(population / wpc).
- Selection is MAP-Elites truncation per niche; swap in tournament/UCB in
  `run_engine` if you prefer.

---

# Corpus Studio (second binary)

`corpus_studio` automates the v1.5 corpus-LM pipeline (the tutorial) as a live
stepper. Build it alongside bot-arena and run:

```bash
cargo run --release --bin corpus_studio
```

## What it does

- **Dataset picker** — choose a corpus folder; **Stage to temp** copies it into a
  fresh `/tmp/corpus-studio/corpus-<ts>` working dir so runs are isolated.
- **Preprocessing tool slot** — point it at your corpus-handling tool. It runs
  first (stage 0) with an arg template; `{in}` is your corpus, `{out}` is
  `<output>/preprocessed`. **Contract:** your tool reads `{in}` and writes a
  normalized corpus (`.md`/`.txt`) into `{out}`; stages 1–6 then read from there.
  Toggle it off to skip.
- **Binaries dir** — point at your `v1.5-complete/bin` (the compiled C pipeline).
- **Parameters** — sliders for BPE merges, n-gram order, Word2Vec dim/epochs,
  transition niches, and tiny_gpt steps/dim/layers.
- **Live stepper** — the Pipeline page shows all seven stages; each lights up
  (pending ○ / running ◐ + spinner / done ✓ / failed ✗) and streams its output
  into an expandable per-step log as it runs.
- **Run all / Step / Stop** — run the whole pipeline, advance one stage at a time
  while you watch, or halt.
- **Sticky settings** — every field (corpus, tool, binaries/output dirs,
  `CORPUS_EXT`, the preprocess toggle, and all parameter sliders) is saved to
  `<user-config>/corpus-studio/settings.conf` on close and restored on the next
  launch, so you can pick up exactly where you left off. **Reset** (top-right)
  restores defaults and forgets the saved file, so the reset sticks across
  restarts too. (Round-trip verified by `cargo test`.)

The stages it orchestrates (verified to run end-to-end against the v1.5 binaries):

```
0  your_tool {in} {out}                                   (optional)
1  bpe_train      CORPUS tokenizer.bin <merges>
2  ngram_train    CORPUS tokenizer.bin model.bin <max_n>
3  word2vec_train CORPUS tokenizer.bin vectors.bin <dim> <epochs>
4  sentence_pool  CORPUS tokenizer.bin vectors.bin paragraph_vectors.bin
5  transitions    paragraph_vectors.bin transitions.bin <niches>
6  tiny_gpt train CORPUS tokenizer.bin gpt.bin <steps> <dim> <layers>
```

`CORPUS_EXT` from the Dataset page is passed to every corpus-reading stage, so you
can point the pipeline at a repo (`.c,.py,...`) instead of just markdown.

## How it pairs with bot-arena

Corpus Studio produces `vectors.bin` / `transitions.bin` — exactly the latent
space bot-arena's swarm uses for worker purposes. Build the artifacts here, then
ground the arena's workers in them. Studio makes the latent space; the arena
deploys it.

## Corpus tool: create-md-from-web.py (wired)

The uploaded HTML→Markdown tool ships in `tools/create-md-from-web.py` and is the
default stage-0 preprocessor. It takes `input_dir output_file [-j N]`, recursively
runs `html2text` on every `.html`/`.htm`, strips images, flattens links, collapses
blank lines, and writes one combined markdown file.

Studio's stage 0 calls it as `python3 tools/create-md-from-web.py {in} {out}/corpus.md -j 4`
(Python scripts are auto-detected and run via `python3`). `{in}` is your HTML
corpus; `{out}` is `<output>/preprocessed`; stages 1–6 then read that dir.
**Needs the `html2text` CLI** (`pip install html2text`).

Verified end to end here: a sample HTML tree → the tool → `preprocessed/corpus.md`
→ `bpe_train` consumes it. For a different tool, just edit the arg template; the
`{in}`/`{out}` substitution and the `.py`→`python3` handling are generic.
