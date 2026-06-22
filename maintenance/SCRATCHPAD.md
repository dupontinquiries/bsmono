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
- **B5 — Nix tooling (discussion open).** `idea (user interested; add as we go)`
  Strong fit for this repo's pain: many system deps (Qt6/gstreamer/vulkan/gtk4),
  the dual 1.75+1.96 toolchain, and cross targets (aarch64/x86/riscv) all want
  reproducible pinning. A `flake.nix` dev shell + cross toolchains would make the
  build host reproducible; tradeoff is the learning/maintenance curve, and
  flatpak already covers app *distribution*. User is fine adding nix artifacts
  incrementally (like the gitmind fixture). Next: seed a starter `flake.nix`
  (devShell + both rustc versions + the apt deps as nix pkgs) for discussion.

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
- **F6 — gitmind vs nested `.git` (cargo makes repos by default).** `idea`
  `cargo new` creates a `.git` per crate; in a monorepo we don't want nested
  repos. Quick upgrades to weigh: set `[cargo-new] vcs = "none"` (cargo config)
  so new crates don't spawn `.git`; and check how `gin import` treats any nested
  `.git` (ignore / submodule / choke) — possibly teach it to skip them.

## G. Kits, FFI boundaries, shared libs (planning)

- **G1 — Kits / FFI boundaries / shared libs.** `in-progress`
  First-pass plan written: **`docs/architecture/KITS_AND_BOUNDARIES.md`** (FFI
  inventory + keep/oxidize/kit-ify + build-shape impact). Key seeds:
  - *Shared-lib candidates:* `peek` (already shared at tools/peek; lib API
    `view::read_to_string`), `gitmind-core` (CLI+GUI share it — the "can't drift"
    model), network-sim/`snapclone-net-intel` (→ a test harness, cf. E1),
    rustcheck's check logic, a future `snapcore-protocol` (ops-doc) crate.
  - *FFI boundaries already in the tree:* snapclone Rust↔C++ DataBus (canonical;
    oxidation target C1), voiceterm Rust↔Qt (qmetaobject cdylib), gitmind-gui
    Rust↔Qt, photosync C++ gui ↔ Rust daemon, unendlich Rust↔Vulkan/shaders.
  - *Kit candidates (from the original handoff):* experimentkit, gesturekit
    (bot-arena), the looking-glass pattern as the extraction discipline.
  - *New (this round):* unendlich's **linkme autoloader** is the **registry-kit**
    (highest-value, pure-Rust, self-contained lift); build **variants** —
    integrated snapclone+unendlich *sharing the DHT* vs *lean* playtester builds
    without snapclone bloat — reshape the build via a `dht` cargo feature + a
    top-level `shared/` layer. Sequencing is in the doc.

## H. Deployment / install / uninstall

- **H1 — Clean, low-effort teardown of system installs.** `idea`
  Once systemd units + virtual keyboards get installed on a host, undoing them
  should be smart and not manual. Track what each deploy installs (units, vkbd,
  firewall rules) so a single `--uninstall` reverses it. Make install/uninstall
  a **deployment stage** in the build scripts (alongside build/flatpak).
  snapclone already ships systemd units — model the teardown there first.
- **H2 — Phone installer GUI + self-hosted distribution.** `idea`
  Ship an installer graphic for the phones that pulls from a good CDN ref / IP
  hosting a basic server (**dufs** + solid firewall patches) and lets users pick
  which actions to apply. Dovetails with the ops-doc self-hosted flatpak repo.

## Build-shape & hardware reality (this round)

- **Targets corrected** in `manifest/targets.toml`: build HOST is **aarch64
  (Asahi, 16K pages)**; phone fleet is **aarch64 musl (FP5 pmOS/Alpine, ~4K
  pages)**; **x86** and **riscv** are CROSS targets (no x86 hardware) — games
  must reach x86 (added to unendlich) and eventually riscv. Mind 16K-vs-4K page
  assumptions. This is *why* cross + snapclone oxidation matter.
- **LOC north-star:** ~600K now; the kits/FFI work should trend it **down**
  (dedup, fewer copies) — a maintainability goal, not size for its own sake.
- **`.spv` shaders:** added a `*.spv` gitignore catch-all (stop runtime-generated
  shaders littering). Pre-existing `assets/shaders/*.spv` stay tracked until the
  shader pipeline compiles-on-build; build/test scripts should also run from a
  scratch PWD so artifacts don't land in the repo. (TODO with build-wiring.)
- **Stale monorepo unendlich:** the monorepo copy is a snapshot; the geom_probe
  fix happened in the *standalone* unendlich repo, so verify here still shows
  `PROBLEMS(5)`. Open: how to sync per-project fixes back (subtree pull? re-import?).

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

## Portfolio LOC snapshot (~600K total, tokei on the user's aarch64 host)

unendlich 303K · voiceterm 135K · bo-suite 75K · logscope 50K · snapclone 42K ·
ambient-learner 22K · bot-arena 13K · av1tuner 10K · gitmind 9K · photosync 1.1K ·
rustcheck/peek/drift small. Plus ~92K C++, ~56K QML, ~14K Python.
