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
- **peek** → `tools/peek` is canonical. (See deferred task below.)

## Deferred build-wiring tasks

These are intentionally **not** done in the skeleton stage, because they edit
build graphs that can only be validated by compiling — which happens on
hardware, not here. Each is a tracked task for the build-wiring stage:

1. **peek dedup repath.** `apps/gitmind` still embeds its own `crates/peek`
   (peek is "copied around" because it's broadly useful — not divergent). Left
   intact so gitmind's internal workspace stays coherent. Wire-up task: replace
   it with a path dependency on `../../tools/peek` and delete the embedded copy.
2. **opendht repath in snapclone.** `system/snapclone`'s CMake referenced
   `third_party/opendht` (now removed). Repoint it to `../../vendor/opendht`.
3. **No single cargo workspace.** Each sub-project keeps its own workspace; the
   unified build is the orchestrator in `build/` driven by `manifest/`.

## Excluded as cruft / artifacts (rebuildable)

`.git` working copies (history archived separately), `target/`, `__pycache__/`,
`node_modules/`, `dist/`, the 116 M vendored Rust toolchain (kept as a CI
artifact, not source), `logscope/scene-view-linux-x64` (prebuilt), and the
ephemeral logscope render PNGs (archived to you, then wiped).
