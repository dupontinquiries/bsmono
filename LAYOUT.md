# Layout, branching, and deferred tasks

## Taxonomy

Projects are grouped by their primary role, not by language:

- **apps/** — user-facing applications. Some contain more than one component
  (e.g. `bo-suite` ships the `bo-controller` app *and* the `bo-mux` daemon);
  the project is filed by its primary nature and the nuance is recorded in
  `manifest/projects.toml`.
- **system/** — system-tier services that daemonize: `snapclone` (DHT /
  identity / relay, the canonical Rust↔C++ FFI) and `photosync`.
- **tools/** — the dev-loop and the three verification *suspenders*
  (`peek`, `drift`, `rustcheck`).
- **vendor/** — vendored third-party. Currently only `opendht`, refreshed to
  current upstream (replaces the stale copy that was embedded in snapclone).

Out-of-scope apps (a Next.js game clone, an HTML bestiary game, a
silence-splitter script) are intentionally **not** in this tree. They indicate
adjacent surfaces — real-time multiplayer/netcode, game content, audio/video
segmentation — and can be folded in later once the kits exist.

## Branching model

- `main` — the unified monorepo (this tree). All cross-project work happens here.
- `project/<name>` — one orphan branch per project, each holding that project
  **standalone** at the branch root. Purpose: isolated work, clean boundary
  preservation, and easy diffing against the original drops. Git de-duplicates
  identical blobs, so the branches cost almost nothing on top of `main`.

Fresh history: snapclone's original `.git` (the one worth keeping) is preserved
in a separate archive, not grafted here. If you later want its history on
`project/snapclone`, it can be grafted in without disturbing `main`.

`.git` and `.gitmind` run side by side. `.gitmind/` is scaffolded but must be
initialised/populated by `gin` (gitmind CLI) on hardware — it is not built here.

## De-duplications applied

- **opendht** → single copy at `vendor/opendht` (current upstream). The stale
  embedded `snapclone/third_party/opendht` (and its nested `.git`) was dropped.
- **rustcheck** → single source copy at `tools/rustcheck` (`.py` + `.rs`). The
  two extra copies and the ~10 M of bundled binary / py-env were dropped.
- **peek** → `tools/peek` is canonical. (See build-wiring status below.)

## Build-wiring status

The unified build is an **orchestrator** (`build/build.sh`) over each project's
own build system, driven by `manifest/projects.toml` + `manifest/targets.toml`
(no single cargo workspace — nested workspaces + C++/cmake can't be one). It
prints the plan by default and runs host-target cargo builds with `--run`;
cross targets and C++/cmake/hybrid projects print what they'd run (they need
cross sysroots / Qt6 / cmake deps — the hardware stage). `build/flatpak.sh` is
the Flatpak stage stub.

- ✅ **opendht repath in snapclone (done).** `system/snapclone`'s CMake set
  `OPENDHT_SUBMODULE_DIR` to the removed `third_party/opendht` (so it would
  FATAL_ERROR). Now it prefers `../../vendor/opendht`, with the legacy submodule
  names as fallback. (Full validation = a cmake configure on a Qt6 host.)
- ✅ **orchestrator (done).** `build/build.sh` fleshed out; `--run --only peek`
  builds for real, proving the cargo dispatch end-to-end.
- ⏸ **peek dedup repath (deferred — not a path swap).** The two peeks have
  **diverged in structure**, not just location: `tools/peek` is bin-centric
  (`main.rs`/`peek2.rs`), while `apps/gitmind/crates/peek` is a lib+bins
  (`lib.rs`+`view.rs`+`bin/`) and **gitmind-gui depends on the library**
  (`peek::view::read_to_string`). Deduping needs `tools/peek` to expose the same
  lib surface, then a repoint — and verifying it needs a Qt6 host to build the
  GUI. Tracked as a reconciliation task, not a quick win.

## Excluded as cruft / artifacts (rebuildable)

`.git` working copies (history archived separately), `target/`, `__pycache__/`,
`node_modules/`, `dist/`, the 116 M vendored Rust toolchain (kept as a CI
artifact, not source), `logscope/scene-view-linux-x64` (prebuilt), and the
ephemeral logscope render PNGs (archived to you, then wiped).
