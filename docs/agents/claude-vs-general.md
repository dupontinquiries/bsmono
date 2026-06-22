# Claude-specific vs general-LLM — and the logscope tree approach

The agent docs in this repo (catalogued in [[INDEX]]) mix two things: guidance
that holds for **any** LLM working on this code, and guidance that only applies
because the agent is **Claude in this sandbox**. This file separates them, then
shows how the workflow maps onto **logscope's tree-based development** model.

Cross-links use ZBracket syntax (`[[name]]`) so voiceterm renders this set as a
live note-graph (see "ZBracket navigation" below).

## 1. General — portable to any LLM agent

These are project discipline, independent of which model or tools you have.

- **Handoff-first.** Write a handoff a fresh agent reads, then continues; name
  the workflow the user values so it survives the context reset. (logscope's
  render loop is explicitly flagged "preserve this"; do the same for any prized
  loop.)
- **DECISIONS discipline.** Record decisions and their rationale as you go, so
  the *why* outlives the chat. Pairs with the scratchpad ([[SCRATCHPAD]]).
- **Drift's four rules** (from `tools/drift/CLAUDE.md`, generalized):
  1. *Boundaries only* — annotate/instrument only seams where two parts change
     independently (Rust↔QML, service↔service, schema↔data). Internal code
     recompiles together; the compiler already covers it.
  2. *Every feature needs a verifier* — an observable signal that confirms or
     refutes its claim, or it's decoration.
  3. *No fantasy features* — resist scope bloat back into a feature pile.
  4. *Respect module purity* — don't inject logging/`crate::` macros into
     modules the author deliberately kept out of the dependency graph.
- **One core, thin frontends ("can't drift").** gitmind's GUI `Backend` calls
  the same `gitmind-core` the CLI runs, so a core fix fixes both; snapclone's
  DataBus and voiceterm's Qt cdylib follow the same shape. Keep logic in the
  core; keep frontends thin.
- **Be honest about what a check proves.** Our suspenders model this: rustcheck
  `--fast` is trustworthy; the peek wiring grep over-reports (candidates, not a
  gate); drift is spec-only. State the confidence, don't oversell.
- **Match output to the request.** Don't overproduce artifacts; be decisive.

## 2. Claude-specific — this environment and our tooling

True because the agent is Claude in this sandbox; another setup may differ.

- **The sandbox resets between sessions** and has **no Rust toolchain** by
  default. Anything installed is ephemeral; only committed files persist. The
  user's offline 1.96 dist installs without network; pair with 1.75 (see
  [[TOOLING]]).
- **Shell is sh/dash, not bash** — no `[[ ]]`, no arrays. Put any bash-ism
  (e.g. the render loop) in a `#!/bin/bash` *file* under `/home/claude/` and run
  it. `pip` needs `--break-system-packages`. `rsync` is absent (use `cp -a`).
- **Network is allow-listed** (crates.io, github, pypi, ubuntu). GitHub
  *release-asset* binaries and `static.rust-lang.org` are blocked, so prebuilt
  tools can't be fetched — `cargo install` or apt instead.
- **Static suspenders** live at `verify.sh` (root): rustcheck `--fast` as the
  trustworthy gate, peek wiring as a candidate-lister, drift deferred. Run them
  before claiming a tree is sound.
- **Deliverables** go to `/mnt/user-data/outputs` and are surfaced with the
  file-presentation tool; scratch work stays in `/home/claude`.
- **The render-verification loop** (logscope §3) is Claude-shaped: Xvfb →
  `scene-view` → ImageMagick `import` → montage → view the PNG inline → judge →
  iterate. The "view a PNG inline" step is a Claude tool affordance.
- **gitmind side-by-side** is our running experiment ([[gitmind-eval]]): the
  `gin` CLI builds and round-trips byte-faithfully; `.gitmind` is a permanent
  *reproducible* fixture.

## 3. logscope's tree-based development approach

logscope renders an agent/conversation log as a navigable 3D **mind palace**:
each turn is a node on a strand, dispatched into themed scenes, with the
image/render loop as the per-change verifier. As a development *model* it says:
treat the work as a **tree of decision-nodes**, each carrying its own observable
verifier, rather than a flat transcript.

Adapting the workflow onto it:
- Handoffs and DECISIONS are the **strand**; each node is a decision + its
  verifier (a render, or a `verify.sh` run).
- Navigate by branch (what changed where) instead of scrolling a log.
- A general LLM without the 3D renderer can still adopt the **"node + observable
  verifier"** structure — that's the portable core of the idea.

## ZBracket navigation (web2qml / voiceterm)

voiceterm links notes with `[[double-bracket]]` ZBrackets and builds a live
note-graph from them (`apps/voiceterm/src/zbrackets.rs`, `note_graph.rs`).
**Convention for this docs set:** link sibling docs and project handoffs with
`[[name]]` so that, opened in voiceterm, [[INDEX]] ↔ this file ↔ each project
handoff form a walkable graph. Use the doc's filename (sans `.md`) as the
target.
