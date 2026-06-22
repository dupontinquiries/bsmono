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
