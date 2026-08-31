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
- **`maintenance/tools/fleet_build.sh`** — does every app still compile against
  the shared crates? The apps are separate cargo workspaces, so there is no one
  `cargo test` that covers them.
  - **Artifacts do NOT go in `apps/*/target` any more.** One shared cache at
    `~/.cache/fleet-build/target`, or `/dev/shm` when it fits, because each run
    used to start from a cold per-iteration tree and write ~40 GB.
  - The RAM cache is automatic and refuses when `/dev/shm` is too small —
    tmpfs is swap-backed, so a tight fit would page to the SSD in small random
    writes, which is worse than the sequential ones it avoids. `RAMCACHE=0`
    forces disk.
  - It prints where stale `apps/*/target` directories are and what they cost.
    It never deletes them.
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

Run by hand, on one app at a time (`python3 maintenance/tools/X.py apps/foo`):

- **unhosted_check** — modules nothing calls. **Trust after hand-checking**,
  and hand-check with a WHOLE-TREE grep rather than the tool's own method.
  It has been wrong three times, each because a caller can exist in a way it
  could not see: aliased imports, callers in a sibling crate, and callers in
  C++ across an FFI boundary. Its first run on gitmind reported 35 modules of
  which 7 of the first 8 were false. All three are fixed; the lesson is that
  a fourth kind of caller probably exists.
- **testonly_check** — public functions whose only callers are tests. This is
  the finer version and catches what the above misses, because the function
  lives in a module that IS reachable. Single digits per app, and the first
  finding hand-checked was a real bug (a delivery ack nothing called, so a
  sent snap could only ever time out).
  **Point it at `apps/<app>/crates`, not one crate.** A caller in a sibling
  crate is still a caller: logscope's four crates scanned separately report 22
  findings, scanned together 9, and the thirteen difference are live functions
  that a tidy-up from the narrow list would delete. It used to be too slow to
  run any other way — the span scan was quadratic — which is a good example of
  a tool's cost silently choosing its scope, and the scope choosing the answer.

Both answer "is this reachable", not "is this correct". A hit is a question.
Four different answers are normal: superseded work to delete, unfinished work
to wire, speculative design the app never adopted, and library API whose
consumer nobody has written yet — and only reading tells you which.

## gitmind (git + gitmind side-by-side)

This repo runs git and **gitmind** in parallel as an evaluation. The gitmind
object store (`.gitmind/store.redb`) is gitignored and reproducible:

- **`maintenance/gitmind-sync.sh`** — builds the `gin` CLI if needed and rebuilds
  the store from the git graph. Run after clone to materialize `.gitmind`.

See `maintenance/gitmind-eval.md` for what's been verified.

## If you are an agent

Read [CLAUDE.md](CLAUDE.md) first. It is the operating manual: session capture,
the build wrapper that halves target directories, the crates whose native
dependencies need a feature flag here, and the checks to run before committing.
This file explains what the tools ARE; that one says which to reach for.

## First things to run

```sh
./verify.sh                     # static suspenders over the whole tree
build/build.sh                  # see the build plan (no compiling)
build/build.sh --run --only peek    # prove a real cargo build
maintenance/gitmind-sync.sh     # (optional) materialize the gitmind store
```

Build note: prefer `maintenance/tools/cargo-ailite check` over bare `cargo
check` — same result, 42% smaller target directory (measured), which is the
resource that actually runs out. `maintenance/tools/ailite-cache` makes a warm
target survive a sandbox reset (8830 ms → 39 ms, verified). Details and the
one claim that did NOT reproduce are in
[maintenance/SANDBOX_BUILD_LIMITS.md](maintenance/SANDBOX_BUILD_LIMITS.md).

Toolchain note: you'll want Rust **1.96** and **1.75** both available (some
benches need rustc < 1.81). System build deps (Qt6, Vulkan, GStreamer, GTK4, …)
are listed in [maintenance/TOOLING.md](maintenance/TOOLING.md).
