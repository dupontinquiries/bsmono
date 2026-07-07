# commands/

Workflow helper scripts that aren't part of the engine itself but make
day-to-day iteration smoother. Survives across Claude zip drops
(`update_from_claude_zip.sh` preserves this directory).

**For tool usage patterns** (which tool when, reading the output, working
with Claude during perf sessions), see [`../docs/WORKFLOWS.md`](../docs/WORKFLOWS.md).
This file documents the *setup* and *update* side of the workflow;
WORKFLOWS.md documents the *measure and analyze* side.

## Recommended directory layout

```
~/unendlich-development/
├── unendlich-game/           ← this repo (the git working tree)
│   ├── .git/
│   ├── Cargo.toml            ← workspace manifest (c.12a+)
│   ├── commands/             ← these scripts
│   ├── crates/               ← workspace member crates (c.12a+)
│   │   ├── unendlich-engine/
│   │   ├── unendlich-shared/
│   │   ├── unendlich-launcher/
│   │   └── unendlich-analyze/
│   ├── tests/specs/
│   └── docs/
└── claude-zips/              ← archive of every zip Claude sends
    ├── unendlich-v0.24.2c.10.zip
    ├── unendlich-v0.24.2c.11.zip
    ├── unendlich-v0.24.2c.12-a.zip
    └── ...
```

Why this layout:

- The git repo (`unendlich-game/`) has Cargo.toml (workspace manifest)
  at its root — standard Rust workspace layout.
- The `crates/` directory holds independent crates that share a target
  directory and Cargo.lock.
- Zips live in `claude-zips/` (a sibling, not inside the repo), so
  they don't pollute `git status` or risk being wiped by the update
  script.
- `~/unendlich-development/` is a single containing folder for the
  whole workflow.

## Standard update workflow

When Claude sends a new `unendlich-vX.zip`, save it to `claude-zips/`
then from inside `unendlich-game/`:

```bash
# Apply the new drop. Bare filename — script knows to look in ../claude-zips/.
./commands/update_from_claude_zip.sh unendlich-v0.24.2c.12.zip

# Review what changed.
git diff --stat   # or gitui

# Commit and tag.
git add -A && git commit -m "Update to v0.24.2-c.12"
./commands/tag_current_version.sh

# Smoke test the new build via the launcher.
./commands/launcher_smoke.sh
```

## Scripts

### `update_from_claude_zip.sh <zip>`

Wipes the working tree (preserving `.git/` and `commands/`) and unpacks
the zip in its place. The zip arg can be:

- A full path: `./commands/update_from_claude_zip.sh /abs/path/to/foo.zip`
- A bare filename: `./commands/update_from_claude_zip.sh foo.zip`
  - Searched in `../claude-zips/`, then `~/Downloads/`, then `./`

Validates the zip structure before destroying anything, so a corrupt
zip won't leave you with no tree. Refuses to run if:
- cwd isn't a git repository.
- Cargo.toml is missing or doesn't say `name = "unendlich"`.
- The zip doesn't contain a top-level `unendlich/` directory.

Edit `ALWAYS_KEEP` at the top of the script to add other entries you
want preserved across updates.

### `tag_current_version.sh`

Reads `version = "X.Y.Z"` from Cargo.toml and creates a `vX.Y.Z` tag
on HEAD. Refuses to clobber an existing tag pointing at a different
commit.

Use after committing a Claude drop, so `unendlich-launcher --version
@vX.Y.Z` resolves correctly later.

### `launcher_smoke.sh`

Builds the launcher and runs `tests/specs/rd2_smoke.unendtest` against
`@HEAD`. The "is everything working" smoke test. First invocation is
slow (~60-90s for the fresh cargo build); subsequent ones are fast
(cache hits).

## Adding new scripts

Drop them here as `*.sh`, `chmod +x`, document above. They survive
zip updates automatically. Anything you want preserved at the repo
root level (rather than inside `commands/`) should also be added to
`ALWAYS_KEEP` in `update_from_claude_zip.sh`.
