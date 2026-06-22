# NEWCOMER_DEVELOPER.md

Start here if you're new to this monorepo (or returning after a while). It maps
the build system and tooling and tells you what to run first. Deeper layout and
rationale: [LAYOUT.md](LAYOUT.md). Agent/AI docs: [docs/agents/INDEX.md](docs/agents/INDEX.md).

## What this repo is

A unified workspace for ~12 personal projects (Rust + some C++/QML/Python)
targeting desktop Linux, phones (Pinephone/postmarketOS/Plasma Mobile), and
Alpine-on-server. It is **not one cargo workspace** — each project keeps its own
build system; a thin orchestrator builds them all per the manifests.

```
apps/      end-user apps (unendlich, voiceterm, bo-suite, logscope, …)
system/    system services/daemons (snapclone, photosync)
tools/     the "suspenders" — static checkers (peek, drift, rustcheck)
vendor/    vendored third-party (opendht)
manifest/  projects.toml + targets.toml  ← drive the build
build/     the orchestrator (build.sh) + flatpak stage (flatpak.sh)
maintenance/ scratchpad backlog, tooling notes, gitmind eval + sync
docs/      docs, incl. docs/agents/ (AI-agent docs)
verify.sh  run the static checks over the whole tree
```

## The manifests (the source of truth)

- **`manifest/projects.toml`** — one entry per project: `kind` (cargo / hybrid /
  cmake / python+rust), `role`, `flatpak`, `runtime`, `targets`, notes.
- **`manifest/targets.toml`** — the cross targets: `desktop`
  (x86_64-gnu), `phone` (aarch64-gnu), `alpine` (x86_64-musl).

Edit these to add a project or target; the tools below read them.

## The build system tools

- **`build/build.sh`** — build orchestrator. Reads the manifests, expands
  project × target, and **prints the plan** by default. `--run` actually builds
  host-target cargo projects; cross targets and C++/cmake/hybrid projects print
  what they'd run (they need cross sysroots / Qt6 / cmake deps).
  - `build/build.sh` — plan everything
  - `build/build.sh --only snapclone` — one project
  - `build/build.sh --target phone` — one target
  - `build/build.sh --run --only peek` — really build (host cargo)
- **`build/flatpak.sh`** — Flatpak packaging stage (stub): for each
  `flatpak != false` project, generate offline cargo sources + a manifest using
  its `runtime`, then `flatpak-builder`. Runs on a host with the flatpak tools.

## The suspenders (static checks) — and how far to trust them

Run via **`./verify.sh`** (no compiler needed):

- **rustcheck** (`tools/rustcheck`, `--fast`) — Tier-0 static. **Trustworthy**:
  low false-positive; catches real syntax/structural breakage.
- **peek** (`tools/peek`, wiring heuristic) — flags possibly-unwired modules.
  **Over-reports** (it can't see `mod`/`use`/re-export wiring) — treat as
  *candidates*, not a gate. The real peek binary at build time is the authority.
- **drift** (`tools/drift`) — boundary-drift checker, **spec-only today**
  (playground compiles; the `cargo drift` subcommands aren't implemented).

## gitmind (git + gitmind side-by-side)

This repo runs git and **gitmind** in parallel as an evaluation. The gitmind
object store (`.gitmind/store.redb`) is gitignored and reproducible:

- **`maintenance/gitmind-sync.sh`** — builds the `gin` CLI if needed and rebuilds
  the store from the git graph. Run after clone to materialize `.gitmind`.

See `maintenance/gitmind-eval.md` for what's been verified.

## First things to run

```sh
./verify.sh                     # static suspenders over the whole tree
build/build.sh                  # see the build plan (no compiling)
build/build.sh --run --only peek    # prove a real cargo build
maintenance/gitmind-sync.sh     # (optional) materialize the gitmind store
```

Toolchain note: you'll want Rust **1.96** and **1.75** both available (some
benches need rustc < 1.81). System build deps (Qt6, Vulkan, GStreamer, GTK4, …)
are listed in [maintenance/TOOLING.md](maintenance/TOOLING.md).
