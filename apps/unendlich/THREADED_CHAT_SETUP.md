# THREADED_CHAT_SETUP — parallel feature work across web chats

A repeatable way to build a large codebase in parallel using several Claude
**web chats** (no Claude Code / IDE / VM needed) without losing track of who's
doing what. Hand this file to a new Claude to set the whole thing up.

## The one idea

Separate the work **by area, in the code and docs** — not by parallel chats that
*you* have to route in your head. Keep the separation; move the coordination into
repo artifacts. Concretely: one **coordinator** chat holds the map and does all
merging; several **worker** chats each own one area and only build small,
self-verifying pieces; everything moves as **zips and patches**. You stay the
operator and the final integration authority.

Why it works: if every worker only ships *pure modules* (logic in, data out — no
renderer/GPU/IO imports), their work composes additively and merges almost never
conflict. The hard, unverifiable integration (wiring those modules into the
engine/renderer/UI) is batched to you, on your real machine, where a compiler and
the real test suite exist.

## Roles

- **You (operator).** Dump ideas to the coordinator; carry zips/patches between
  chats; run the integration build/tests on your machine; decide priorities.
- **Coordinator (one chat).** Owns `OWNERSHIP.md` + `BACKLOG.md`. Decomposes your
  ideas into *core* (sandbox-verifiable) vs *wiring* (needs your machine), assigns
  disjoint batches, issues worker briefs, merges returned patches, verifies them,
  prunes overlap, checks items off, emits the next base + an integration checklist.
- **Workers (N chats, one per area).** Each reads its brief, builds its assigned
  pure modules with tests, runs the sandbox harness to GREEN, returns one labeled
  patch. Workers never touch other areas or the integration wiring.

## The artifacts (what travels between chats)

- **Base tree (zip).** One reconciled snapshot every worker starts from this round
  (so the trees can't drift apart). Contains the project + the items below.
- **`OWNERSHIP.md`** — directory→area map + a routing rule + how to split a
  cross-cutting feature. The coordinator routes by *reading* this, not remembering.
- **`BACKLOG.md`** — one owner-tagged board (all per-thread TODOs merged), ordered
  by the core→wiring seam.
- **Per-worker `WORKER_BRIEF.md`** (at the tree root) — role + "you have a
  coordinator" + that worker's task batch + the hard rules + the return steps.
- **`RETURN_KIT.sh`** (root) — `start` snapshots the base; `finish` requires the
  harness GREEN, then writes the labeled patch + a `+`/`~` manifest.
- **`WORKER_ID`** (root, one line e.g. `T02`) — names the returned patch
  (`CHANGES-T02.patch`) so you never rename by hand.
- **`MANIFEST.sha256`** (root) — lets `finish` detect new/changed files.
- A **harness** the worker can run with no full build (see rules) — the GREEN gate.

## The cycle

1. **Coordinator → you:** N worker zips (`worker-0N-<area>.zip`), each = base +
   that worker's brief/ID. Round 1 only: every worker gets the full reconciled base
   (the resync). Later rounds can be lighter (a small patch + new tasks).
2. **You → each worker chat:** upload its zip, no text needed (the brief is
   self-contained). Optional nudge: *"Read WORKER_BRIEF.md and follow it. Be quiet
   and work."* (Reuse existing area chats — they keep their expertise — or start
   fresh; both fine.)
3. **Worker:** `bash unendlich/RETURN_KIT.sh start` → build the modules → `finish`.
   Its reply is just a downloadable `CHANGES-T0N.patch` + a short manifest.
4. **You → coordinator:** drop the N patches in (no text). Suggested local layout:
   a git repo with `changes/CHANGES-T0N.patch`, committing/tagging each round.
5. **Coordinator:** applies all patches to the base (`git apply -p1`), runs the
   harness GREEN, re-verifies every module itself, prunes overlap/dupes/failures,
   checks items off `BACKLOG.md`, and returns: the **new base zip** + a **summary**
   + an **integration checklist** (what to `cargo build`/test + which wiring kits
   to apply on your machine).
6. **You:** apply the new base to your real tree your usual way; run the
   integration tier; report any failures back (each new failure class becomes a new
   harness rule so it can't recur).

## Rules that keep merges clean (the workflow's constitution)

1. **Workers ship only pure modules** — no `crate::render` / GPU / `ash` / `vk`,
   no wall-clock / RNG, no `.unwrap()`/`.expect()`. Pure ⇒ unit-testable standalone
   ⇒ composable ⇒ conflict-free.
2. **Disjoint areas, disjoint files.** Each worker adds files only under its own
   directory and appends declarations to **one** module file unique to its area
   (e.g. `sim/mod.rs` vs `world/mod.rs` vs `lib.rs`). No two workers touch the same
   file → patches never collide.
3. **Wiring is the operator's, not the workers'.** Anything that needs the renderer,
   shaders, the main loop, descriptors/UBOs, or feature gates only compiles/verifies
   on your machine. The coordinator prepares those as ready-to-apply *kits* for you;
   workers don't attempt them.
4. **The harness GREEN is the ship gate.** `finish` refuses to emit a patch unless
   the sandbox harness passes (delimiters, purity, determinism/hygiene, the modules'
   unit+property tests). The coordinator re-verifies on merge — never trust "it
   passed" without re-running.
5. **Overproduce cores, then prune — never overproduce wiring.** Each brief may
   include *one* clearly-marked OPTIONAL pure core. The coordinator prunes anything
   two workers built twice, that duplicates an existing module, or that fails the
   harness; you prune the rest on taste/perf. Pure cores are isolated and trivially
   droppable; speculative wiring is not, so it's never produced.
6. **Decompose features before routing.** A single idea ("bees") is usually several
   items in several areas (a creature kind + pollination wiring + a climate input +
   an enum). Split into core+wiring per area first; queue each separately.

## Your inbox (steady state)

Send the coordinator your ideas however raw — a stream of consciousness is fine.
The coordinator's standing job: parse the stream, split each idea into pure-core
vs wiring, tag it to an area on `BACKLOG.md`, note cross-area dependencies, and
fold the ready cores into the next round of briefs. You stop having to remember
anything; the board remembers.

## Thinking levels

- **Workers:** standard / "extended" (medium) is plenty. Tasks are tightly
  specified; the budget should go to getting an algorithm's invariants and its
  property tests right, not to open-ended exploration. Don't crank workers to
  maximum — it mostly costs latency/tokens for well-scoped module work.
- **Coordinator:** the highest level you're comfortable paying for. Its work is the
  judgment-heavy part — decomposing vague ideas, spotting cross-area coupling,
  merge/prune decisions, keeping the ruleset honest. This is where deep thinking
  earns its keep.
- **Integration/debugging passes (with you):** high — diagnosing a real
  build/runtime failure across the whole tree benefits from it.

## Bootstrapping on an existing codebase (what a new coordinator does first)

1. Inventory the tree; identify natural **areas** by directory and concern; write
   `OWNERSHIP.md` (area→paths, the routing rule, cross-cutting examples).
2. Find/define the **sandbox harness**: the subset of checks that run without a
   full build (delimiter balance, a purity/coupling check, determinism/hygiene,
   and standalone unit+property tests on pure modules). This is the GREEN gate.
3. If multiple divergent snapshots exist, **reconcile** them into one base: take
   layout-independent files (pure modules, tooling) wholesale; hand-merge
   layout-specific files (main loop, renderer, shaders, declaration files), never
   blind-overwrite. Verify the base is harness-GREEN.
4. Merge all existing TODOs into one owner-tagged `BACKLOG.md`, ordered cores-first.
5. Pick the first **disjoint** batch (2–3 pure cores per area + one optional),
   write the per-worker briefs, build the worker zips (base + brief + `WORKER_ID` +
   `RETURN_KIT.sh` + `MANIFEST.sha256`), and run the cycle.

### Paste-block to start a coordinator in a fresh chat

> You are the **coordinator** for a multi-chat parallel build (see
> `THREADED_CHAT_SETUP.md`, attached). I'm the operator. Here is the current base
> tree (zip). Read `OWNERSHIP.md` and `BACKLOG.md`. Then: (a) confirm the area map,
> (b) propose the next disjoint round of pure-core tasks (2–3 per area + one
> optional each, no wiring), and (c) when I approve, produce one
> `worker-0N-<area>.zip` per area using the kit. I'll carry zips/patches between
> chats and run integration on my machine. Keep workers to pure modules; prepare
> wiring as kits for me.

## Failure modes this prevents (learned the hard way)

- **Trees drift apart** → every round starts from one reconciled base; workers
  can't fork further than a round.
- **Merge conflicts** → disjoint areas + one declaration file each + pure-only
  modules ⇒ additive patches.
- **"It looked merged but didn't compile"** → wiring (the unverifiable part) is
  never silently merged; it's an explicit, reviewed kit you apply with a compiler.
- **You route an idea to the wrong chat** → you don't route from memory; the
  coordinator routes from `OWNERSHIP.md`, and cross-cutting ideas get split.
