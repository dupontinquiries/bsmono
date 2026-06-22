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
- **A2 — "Claude-specific vs general-LLM" notes file.** `done`
  Written: `docs/agents/claude-vs-general.md` — Claude-specific vs general-LLM
  guidance + the logscope tree-based-development mapping; ZBracket cross-links.
- **A3 — NEWCOMER_DEVELOPER.md (what each build-system tool does).** `done`
  Root-level map of the entry points (verify.sh, build/build.sh, flatpak.sh,
  gitmind-sync.sh), the manifests, the suspenders, and "first things to run".

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
- **B5 — Nix tooling (discussion open).** `idea (user interested)`
  Strong fit for this repo's pain: many system deps (Qt6/gstreamer/vulkan/gtk4),
  the dual 1.75+1.96 toolchain, and cross targets (aarch64/musl) all want
  reproducible pinning. A `flake.nix` dev shell + cross toolchains would make the
  build host reproducible; tradeoff is the learning/maintenance curve, and
  flatpak already covers app *distribution*. Next: sketch a flake (devShell +
  the two rustc versions + the apt deps as nix pkgs) for discussion.

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
- **F4 — gitmind branches for UI/UX base-swap experiments.** `idea`
  When weighing "who has better UI/UX, w2q or snapclone", spike it: use gitmind
  native branches / **`gin graft`** ("field-test another repo on a branch") to
  take one app's UI code as the base for another, let the agent iterate, then
  build/test/feel-out the quirks to inform the decision. graft is built for
  exactly this. Needs a Qt6 host to actually build/feel the result.
- **F5 — Live break-detection hook vs checkpoint checks.** `decided`
  For the **human dev on hardware**: `gin watch` (autosnapshot on save) + a
  build/test hook is cheap (background) and great. For the **agent (me)**: a
  live per-save hook is high-token *and* doesn't survive the sandbox reset — so
  the right model is checkpoint-based: run `verify.sh` (fast static suspenders)
  plus a targeted `cargo check`/test on the crate I touched, occasionally,
  rather than streaming on every save.

## G. Kits, FFI boundaries, shared libs (planning)

- **G1 — Start planning kits / FFI boundaries / shared libs.** `idea (start)`
  Seeds to develop into `docs/architecture/KITS_AND_BOUNDARIES.md`:
  - *Shared-lib candidates:* `peek` (already shared at tools/peek; lib API
    `view::read_to_string`), `gitmind-core` (CLI+GUI share it — the "can't drift"
    model), network-sim/`snapclone-net-intel` (→ a test harness, cf. E1),
    rustcheck's check logic, a future `snapcore-protocol` (ops-doc) crate.
  - *FFI boundaries already in the tree:* snapclone Rust↔C++ DataBus (canonical;
    oxidation target C1), voiceterm Rust↔Qt (qmetaobject cdylib), gitmind-gui
    Rust↔Qt, photosync C++ gui ↔ Rust daemon, unendlich Rust↔Vulkan/shaders.
  - *Kit candidates (from the original handoff):* experimentkit, gesturekit
    (bot-arena), the looking-glass pattern as the extraction discipline.
  - First pass: classify each FFI seam as keep / oxidize / kit-ify, and name the
    shared crates worth lifting to a top-level `shared/` or `crates/`.

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
