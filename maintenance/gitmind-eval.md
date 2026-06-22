# gitmind evaluation (side-by-side with git)

Running log of using gitmind (`gin`) alongside git to build confidence — or
find where it breaks. Append new trials; keep the verdict current.

## Setup

- Built `gin` (the CLI) from `apps/gitmind/crates/cli` with the offline Rust
  1.96 toolchain. `default-members` excludes the Qt GUI crate (the only part
  that needs Qt6) — so a plain `cargo build` of the toolbox succeeds.
- gin's model: **shadow-checkpoint** commands over an existing git repo
  (`snapshot`, `timeline`, `diff`, `watch`, `review`, `maintain`) **and** a
  full native `.gitmind` object store (`import`, `native-commit`, `native-log`,
  `export-native`, `merge`, `rebase`, …) that needs no `.git`.

## Trial 1 — round-trip fidelity on the monorepo (2026-06-22)

Ran against the canonical monorepo `main` (placeholder `.gitmind/` set aside).

| Step | Command | Result |
|---|---|---|
| checkpoint | `gin snapshot` + `gin timeline` | ✅ shadow checkpoint created + listed |
| import | `gin import` | ✅ 4727 objects (17 commits / 444 trees / 4266 blobs), 16 refs, 111.8 MB |
| native walk | `gin native-log` | ✅ walked native commits |
| **rebuild** | `gin export-native /tmp/rebuilt` | ✅ **byte-faithful**: rebuilt HEAD tree `34cc2325…` == original; 4335/4335 files |
| size | `du` | `.git` 84 MB vs `.gitmind` 110 MB (~+30%) |

**Verdict so far: reliable core.** Correctness is strong (identical tree hash
on full reconstruction). The only cost observed is a larger on-disk store than
git's packfile. The upstream "broken" marker refers to the GUI, not the CLI.

## Trial 2 — compression lever (2026-06-22)

Tested the obvious space lever: the chunk store's `ZSTD_LEVEL` (9 → 19; the
format is self-describing, so the level changes without migration and FastCDC
boundaries — hence dedup — are untouched). Measured the 9-vs-19 delta directly
on the repo's bytes, chunked at 64 KiB the way the store chunks them:

| level | total | vs zstd-9 |
|---|---|---|
| zstd-9  | 81.1 MB | — |
| zstd-19 | 79.9 MB | **−1.5%** |

**Below the 5–10% bar → reverted.** The repo is mostly source text (compresses
about the same at 9 and 19) plus already-compressed media. The real 110-vs-84 MB
gap is git's **delta** compression, which a zstd level can't replicate. Logged
"delta compression in the chunk store" as the real (bigger) lever in SCRATCHPAD.
Fidelity is level-independent (zstd is lossless; Trial 1's byte-faithful
round-trip stands).

## .gitmind as a permanent fixture

Made permanent **reproducibly**: `store.redb` is gitignored and rebuilt by
`maintenance/gitmind-sync.sh` (`gin import`). Committing the raw store was
rejected — it would ~double `.git` (redb is incompressible to git and can't
delta) and a committed store would self-reference on the next import. Because
the store is gitignored it is never a tracked git object, so `import` (which
reads the git graph) never re-ingests it. Tracked: `.gitmind/config.toml` +
`.gitmind/README.md`.

## Open questions for further trials

- Does `.gitmind` stay correct after `native-commit` / `merge` / `rebase`
  sequences (not just import/export)?
- Storage growth over many `watch` autosnapshots — does `gin maintain`/`gc`
  keep it bounded?
- `graft` ("field-test another repo on a branch") — relevant to integrating
  projects; worth a trial.
- Does the size gap shrink after `gin maintain` (pack/bloom)?

## How to run it side-by-side on hardware (ongoing)

Keep both live in a repo: normal `git` workflow **plus** `gin watch` for
autosnapshots and/or `gin native-commit` mirroring each git commit. Periodically
`gin export-native` to a scratch dir and diff against the git HEAD tree to keep
proving fidelity. Promote trust only as trials accumulate.
