# LAUNCHER.md — unendlich iteration & launcher workflow

The reference for applying a Claude ship, validating it, tagging it,
A/B-testing it under the launcher, and playtesting it. Future Claude
sessions should read this before doing anything; the workflow is more
specific than "unzip and `cargo run`".

> **Status flags in this doc**
> ✅ = confirmed from working terminal sessions you've shown me.
> 🟡 = inferred from context; correct if a future session sees the
>      actual source.

---

## The pieces

There are four moving parts beyond the engine crate itself.

**`crates/unendlich-engine`** ✅ — the game/engine. Where ship diffs
land. Built via `cargo build -p unendlich-engine --features render,gpu`.

**`crates/unendlich-launcher`** ✅ — a binary crate that runs the
engine against `.unendtest` spec files, captures `events.jsonl`,
emits diag summaries. Invoked as
`cargo run --release -p unendlich-launcher -- <subcmd> <args>`.
Known subcommands: `run`, `diag`. 🟡

**`crates/unendlich-analyze`** ✅ — analysis subcommand suite over the
events stream. Invoked as
`cargo run --release -p unendlich-analyze -- <subcmd>`.
Known subcommand: `shadow_stride`. There are likely others — grep
`fn main` and the subcommand dispatch in that crate when this doc is
refreshed. 🟡

**`commands/` scripts** ✅
- `commands/update_from_claude_zip.sh <ship.zip>` — apply a Claude ship
  on top of the working tree. Safer than bare `unzip -o` because it
  presumably handles overwrite/cleanup of stragglers like
  `assets/shaders/*.spv`. 🟡 (Confirm by reading the script.)
- `commands/tag_current_version.sh` — parses the version from somewhere
  (likely `crates/unendlich-engine/Cargo.toml` or a `VERSION` file) and
  creates the corresponding `git tag`. 🟡

---

## The standard ship-application loop

Every Claude ship should go through this loop. The recipe:

```bash
cd ~/Code/unendlich-development/unendlich-game

# 1. Clean up stragglers from the previous ship.
git checkout -- assets/shaders/ 2>/dev/null
rm -f events.jsonl

# 2. Apply the ship.
./commands/update_from_claude_zip.sh ~/Downloads/<ship>.zip

# 3. Commit & tag.
git add -A
git commit -m "Update to <ship-version>"
./commands/tag_current_version.sh
#   (or manually: git tag -a v0.24.X.Y-<slug> -m "RC<N> — <summary>")

# 4. Build the workhorses (engine + launcher + analyze).
cargo build --release \
  -p unendlich-engine -p unendlich-launcher -p unendlich-analyze \
  --features render,gpu

# 5. Sanity check (fast, ~30 tests covering recent change areas).
./scripts/sanity.sh

# 6. Run the launcher spec for whatever you're investigating.
cargo run --release -p unendlich-launcher -- \
  run tests/specs/<workload>.unendtest --version @HEAD
cargo run --release -p unendlich-launcher -- diag

# 7. Playtest with the current flag mix (see "Playtest" below).
```

Step 5 is the iteration loop's tightest inner cycle — that's the
"spam and move fast" path. Steps 6–7 are post-validation playtests.

---

## Version naming

The git tag convention you use, by example:

| Tag                                  | RC label | Description                       |
| ------------------------------------ | -------- | --------------------------------- |
| `v0.24.76.1`                         | RC42.1   | sub-voxel templates validated     |
| `v0.24.XX-<slug>` (per-ship form)    | —        | per-ship commits, may not get RCs |

When Claude ships a fix, the readme inside the zip should use a
versioned name (e.g., `v0.24.77-evict-chunk-lod-readme.md`), not the
`v0.24.NEXT` placeholder my earlier zips used. Future Claude sessions:
read the latest tag with `git tag --sort=-v:refname | head -1` and
increment from there. The slug is the change in 1–3 hyphenated words.

The RC<N> label appears to be assigned at integration points where a
batch of ships has been validated together. Don't pre-assign RC
numbers in per-ship readmes — let RCs accumulate.

---

## A/B testing with `.unendtest` specs

You've shown this pattern for `chug_capture_rd8.unendtest` and the
`UNENDLICH_SHADOW_CHUNK_STRIDE` env var. Generalized:

```bash
# Baseline: run the spec as-is, capture events.
cargo run --release -p unendlich-launcher -- \
  run tests/specs/<spec>.unendtest --version @HEAD
cargo run --release -p unendlich-launcher -- diag

# Treatment: sed in the flag(s) being investigated.
sed -i 's|UNENDLICH_RENDER_DISTANCE = "8"|\
UNENDLICH_RENDER_DISTANCE = "8"\
UNENDLICH_<NEW_FLAG> = "1"|' \
  tests/specs/<spec>.unendtest

# Re-run.
cargo run --release -p unendlich-launcher -- \
  run tests/specs/<spec>.unendtest --version @HEAD
cargo run --release -p unendlich-launcher -- diag

# Cross-version analyze (if a per-feature analyzer exists).
UNENDLICH_EVENTS_PATH=~/.unendlich/latest-events.jsonl \
  cargo run --release -p unendlich-analyze -- <analyzer-name>

# Always reset the spec at the end.
sed -i '/UNENDLICH_<NEW_FLAG>/d' tests/specs/<spec>.unendtest
```

The `--version @HEAD` argument tells the launcher to label the run
with the current git HEAD. 🟡 The launcher probably also accepts
explicit version arguments like `--version v0.24.76.1` for
historical-version sweeps; haven't confirmed.

**Per-ship `.unendtest` recommendation.** When Claude ships a fix that
should produce a measurable runtime change, the readme should call out
which existing spec is the right A/B target (and what flag to toggle).
If no existing spec stresses the relevant area, that's a one-line
follow-up to file: "add tests/specs/<workload>.unendtest covering
<scenario>." Don't author new specs blind — the schema includes at
least `[env]` and `warmup_ms` 🟡 and probably more.

---

## Build-break recovery

When the engine builds fine but a downstream bin (`gpu_mesh_bench`,
the launcher's helpers) doesn't, the recovery pattern is:

```bash
# 1. Find the line.
grep -n "<the broken pattern>" <file>

# 2. sed-in-place fix.
sed -i 's|<old>|<new>|' <file>

# 3. Re-build with `tail -3` to confirm.
cargo build --release -p unendlich-engine --features render,gpu 2>&1 | tail -3
```

For multi-line inserts where sed-replace isn't ergonomic, use the line
address form:

```bash
grep -n "<unique anchor>" <file>
# Suppose the unique anchor is at line 631:
sed -i '631a\                    <new line content>' <file>
```

After fixing, amend the commit (don't add new commit noise) and
force-update the tag:

```bash
git add -A
git commit --amend --no-edit
git tag -f v0.24.XX.Y -m "<RC label> — <summary>"
```

If the build is still red after the obvious sed-fix, stop sed-ing and
read the actual error — the schema may have changed in ways that need
real edits.

---

## Playtest

The playtest invocation is always the engine binary directly, with the
current flag mix. The mix evolves; the canonical-as-of-RC42.1 mix:

```bash
UNENDLICH_RENDER_DISTANCE=6 UNENDLICH_LOD_BANDS=2,6,6 \
UNENDLICH_GPU_MESHER=2 UNENDLICH_GPU_MESHER_MID=1 \
UNENDLICH_ADAPTIVE_UPLOADS=1 UNENDLICH_SUBMIT_SKIP=1 \
UNENDLICH_SLACK_SCHEDULER=1 UNENDLICH_BIASED_SHELL_WALK=1 \
UNENDLICH_UPLOAD_PRIORITY=1 UNENDLICH_DEVICE_LOCAL_MESH=1 \
UNENDLICH_MULTI_SCALE_CAVES=1 \
  cargo run --release --bin unendlich-world --features render,gpu
```

Tuning targets in the playtest: high FPS, huge render distance,
absence of stutters. Watch the FPS log line and any
`wait_fences_spike` events in `events.jsonl`.

Per-ship readmes should call out which new flag (if any) the playtest
needs. If a ship has runtime knobs, add them to the playtest mix in
the readme.

---

## What Claude should do at the start of every unendlich session

This list exists so future-Claude can pick up where prior-Claude left off
without re-discovering the workflow.

1. **Read this file first** if it's in the zip. (`docs/LAUNCHER.md`.)
2. **Read `README.md`** at the top of the cumulative for the headline +
   "what's open" status.
3. **Read `docs/v0.24.XX-*-readme.md`** entries for any ships layered
   on top of the prior cumulative baseline — these document the most
   recent context.
4. **Read `docs/DESIGN_NOTES.md`** if it exists (it does in the user's
   real tree, was excluded from this cumulative).
5. **`git tag --sort=-v:refname | head -5`** to see the version
   trajectory. Increment from the latest for new ships.
6. **Confirm cargo / rust toolchain expectations** — current is
   `cargo 1.75.0` on `x86_64-unknown-linux-gnu`. The sandbox here
   typically has no toolchain, so test runs must be the user's
   responsibility.
7. **When delivering a zip, structure it to apply via
   `update_from_claude_zip.sh`**: top-level paths
   (`crates/...`, `docs/...`, `scripts/...`, `tests/specs/...`)
   relative to the project root.

---

## What's deliberately not here

- The `.unendtest` schema, the launcher's CLI surface, and the
  analyze subcommand list. These live in the launcher and analyze
  crates' source, which isn't in the cumulative zip. When you next
  see them, append a section to this doc with the schema + full
  subcommand list, and bump the relevant 🟡 markers to ✅.
- The exact contents of `commands/update_from_claude_zip.sh` and
  `commands/tag_current_version.sh`. Same — append when seen.
