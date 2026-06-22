# Scratchpad / backlog

Living capture of jotted ideas, organized as we go. Newest raw notes land in
"Inbox"; once triaged they move into a themed section with a status and a plan.

Status legend: `idea` · `queued` · `in-progress` · `blocked` · `done` · `parked`

---

## Inbox (raw, untriaged)

_(empty — current dump triaged below)_

---

## A. AI-agent documentation unification

- **A1 — Unify the AI-agent docs across projects.** `in-progress`
  Inventory done (~40 docs, heavily clustered in `apps/unendlich/handoff/`).
  Catalog + unification plan now live at **`docs/agents/INDEX.md`**. Next:
  the link scheme so **web2qml/voiceterm users navigate via ZBrackets**.
- **A2 — "Claude-specific vs general-LLM" notes file.** `queued`
  Planned as `docs/agents/claude-vs-general.md`: separate Claude-specific
  guidance from what generalizes, and what maps onto **logscope's tree-based
  development** approach. (Synthesis step — pending greenlight.)

## B. Tooling & maintenance dir (this directory)

- **B1 — Install dev tools.** `in-progress` (see `maintenance/TOOLING.md`)
  Wanted: sccache, mold, tokei + others. Rust preferred for anything we build;
  Python acceptable. Sandbox has **no toolchain and resets each session**, so
  installs here are ephemeral — the durable output is the documented install
  spec for hardware/CI. Release-binary fetch is firewalled in-sandbox.
- **B2 — Dual toolchain 1.75 + 1.96.** `queued`
  User supplied an offline 1.96 dist; 1.75 needed too (e.g. kanal-pre8 wants
  rustc < 1.81). Documented in TOOLING.md.
- **B3 — More "suspender" tooling, Rust-first, in `maintenance/`.** `idea`
  Quick-check common Rust mistakes; expand peek/rustcheck coverage repo-wide.
  Candidate: a small Rust `repo-doctor` that wraps the static checks.
- **B4 — Recommend additional tools, doc in the Claude AI docs.** `done→living`
  See `maintenance/TOOLING.md` (recommended set + rationale).

## C. snapclone FFI → full Rust (oxidation)

- **C1 — Move snapclone's C++ side to full Rust.** `idea / strategic`
  snapclone is the canonical Rust↔C++ DataBus FFI and the best place to start
  oxidizing: replace C++ with Rust, shrinking/eliminating the FFI surface.
  ~92K C++ LOC total in the tree, mostly here. Big, multi-stage; needs a
  component-by-component plan (start with leaf C++ that has a clean Rust analog).

## D. DHT decomposition + Rust rebuild

- **D1 — Pull the DHT apart, rebuild on new Rust portions.** `idea / strategic`
  Aligns with the ops-doc Part 1 (snapcore-daemon + snapcore-protocol over
  D-Bus). Decompose snapclone-dht/microdht/gossip/nat into reusable Rust pieces.
  Dovetails with C1 (oxidation) and the daemon plan.

## E. CI: network simulation + quick-check tooling

- **E1 — Network simulation for CI testing.** `idea`
  Existing material to harvest: `snapclone-network-diag`, `snapclone-net-intel`,
  a `network_sim` module in bo-suite, channel-bench's `--with-cpu-load`.
  → Could become a `maintenance/` tool or a shared test harness.
- **E2 — Quick-check common Rust-crate mistakes.** `queued` (overlaps B3)

## F. gitmind evaluation (side-by-side with git)

- **F1 — Actively use gitmind; run two side-by-side repos.** `in-progress (positive)`
  Trial 1: `gin` CLI builds clean; checkpoint + native import + **export-native
  byte-faithful**. Trial 2: zstd 9→19 saves only **1.5%** (below bar, reverted);
  delta compression is the real lever (see F3). `.gitmind` made a **permanent
  reproducible fixture** — `store.redb` gitignored, rebuilt by
  `maintenance/gitmind-sync.sh`; config + README tracked. Next trials:
  native-commit/merge/rebase correctness, watch+gc storage bounds, `graft`.
- **F3 — Delta compression in gitmind's chunk store.** `idea (real space lever)`
  The 110-vs-84 MB gap vs `.git` is delta compression, not zstd level. A
  cross-chunk/cross-object delta scheme (the store is BLAKE3-addressed, FastCDC
  64K) could close or beat it. Bigger change than a level bump — a real project.

---

## Reliability / trust ledger (what's proven vs assumed)

- `rustcheck --fast` — **trusted**. Caught the one real break (unendlich
  geom_probe.rs:99 unclosed brace); confirmed + fixed by the unendlich session.
- `peek wiring-audit` (grep form) — **over-reports**, candidates only
  (bo-suite 7/7 dead-service flags were false positives). Authority = real peek
  binary at build stage.
- `drift` — **spec-only** today (60-line playground compiles; cargo-drift
  subcommands unimplemented). The boundary-checker we want for connections.
- `gitmind` (`gin` CLI) — **reliable core** (Trial 1). Builds clean; round-trip
  git→.gitmind→git is byte-faithful. GUI crate (Qt) untested/the "broken" part.

## Portfolio LOC snapshot (rust, this tree)

unendlich 303K · voiceterm 135K · bo-suite 75K · logscope 50K · snapclone 42K ·
ambient-learner 22K · bot-arena 13K · av1tuner 10K · gitmind 9K · photosync 1.1K ·
rustcheck/peek/drift small. Plus ~92K C++, ~56K QML, ~14K Python.
