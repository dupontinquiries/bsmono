# ROLLBACK.md — unendlich rollback & version-recovery system

How to get back to a known-good state when a ship breaks, and how to
run/compare against any prior version without disturbing your working
tree. This is the safety net under the "move fast" loop: every ship is
a tagged restore point, so nothing is ever one bad change away from
unrecoverable.

See `LAUNCHER.md` for the forward workflow (apply → build → run);
this doc is the reverse (undo / recover / compare).

---

## The core idea: every ship is a tagged commit

The application loop (`LAUNCHER.md` step 3) ends with:

```bash
git add -A
git commit -m "Update to v0.24.X-<slug>"
./commands/tag_current_version.sh        # → tag v0.24.X-<slug>
```

That tag is a permanent, named restore point. Because every ship is
committed *and* tagged before you build/playtest it, the state right
before any ship is always exactly one tag back. Rollback is never
"reconstruct what it was" — it's "check out the tag."

List the trajectory any time:

```bash
git tag --sort=-v:refname | head -10
```

---

## Rollback levels (least to most destructive)

### 1. Compare without changing anything — launcher `--version`

The non-destructive default. The launcher can run a spec against any
tagged version while your working tree stays on HEAD:

```bash
cargo run --release -p unendlich-launcher -- \
  run tests/specs/chug_capture_rd8.unendtest --version v0.24.76.1
```

Use this to confirm a regression is real ("does the old tag not have
the stutter?") before touching your tree. 🟡 If `--version` builds the
ref on demand (tier-C launcher), this is fully hands-off; if not, it
falls back to the manual checkout below.

### 2. Roll back specific files — `git checkout <tag> -- <paths>`

Surgical. Restores only the named files to a prior version, leaves
everything else on HEAD. Good for "the barrier change regressed but I
want to keep the evict_chunk_lod fix":

```bash
git checkout v0.24.77-evict-chunk-lod -- \
  crates/unendlich-engine/src/render/mesh_buffer_pool.rs
cargo build --release -p unendlich-engine --features render,gpu 2>&1 | tail -3
```

The working tree now has one file rewound; commit it as a new
("revert") commit so history records the decision.

### 3. Roll back everything to a tag — `git reset --hard <tag>`

The full undo. Moves HEAD and overwrites the working tree to match the
tag. **Destructive** — uncommitted changes are lost. Use when a ship is
a dead end and you want to be exactly back at the prior good tag:

```bash
git reset --hard v0.24.76.1
# optionally clean stray build/asset artifacts the ship dropped:
git checkout -- assets/shaders/ 2>/dev/null
rm -f events.jsonl
```

After this, `git tag --sort=-v:refname | head -1` should read the tag
you reset to. Rebuild and you're on known-good ground.

### 4. Record an undo without losing history — `git revert`

When you want history to show "we tried X and backed it out" rather
than erasing it. Creates a new commit that inverts a previous one.
Non-destructive; preferred when others might pull the branch or when
the failed experiment is itself informative.

---

## Fixing a ship in place (before it's "real")

If a ship's commit is the tip and you just need to patch it (e.g., a
build break — see `LAUNCHER.md` "Build-break recovery"), amend rather
than stacking fixup commits, and force-move the tag:

```bash
git add -A
git commit --amend --no-edit
git tag -f v0.24.X.Y -m "<RC label> — <summary>"
```

This keeps one clean commit per ship and the tag pointing at the
corrected state. Safe because these tags are local and you haven't
published them.

---

## Finding *which* ship introduced a regression — `git bisect`

When a regression appeared somewhere across many ships and you don't
know which, binary-search the tags with the launcher spec as the
good/bad oracle:

```bash
git bisect start
git bisect bad  v0.24.78-barrier-tighten      # known bad (current)
git bisect good v0.24.40-greedy-mid           # known good (older)

# git checks out the midpoint. Build + run the spec to judge it:
cargo build --release -p unendlich-engine -p unendlich-launcher --features render,gpu
cargo run --release -p unendlich-launcher -- \
  run tests/specs/chug_capture_rd8.unendtest --version @HEAD
cargo run --release -p unendlich-launcher -- diag
#   ...inspect diag, then mark:
git bisect good    # or: git bisect bad

# repeat until git names the first bad commit, then:
git bisect reset
```

~log₂(N) builds to find the culprit across N tags. The `.unendtest`
spec makes "good or bad?" an objective check (a perf threshold in the
diag output) rather than a subjective playtest call where practical.

---

## Recovery checklist when a ship breaks a playtest

1. **Don't panic-edit.** The prior good tag is intact.
2. Reproduce under the launcher (`run <spec> --version @HEAD` + `diag`)
   so you have a measured before/after, not a vibe.
3. Confirm the prior tag is clean: same spec `--version <prev-tag>`.
4. Decide scope: one file (level 2), or whole tree (level 3).
5. If it's a quick fix, amend + force-tag (don't stack commits).
6. Re-run `./scripts/sanity.sh` before re-tagging — catch build/logic
   breaks before they reach a playtest.

---

## What's inferred

🟡 The exact behaviour of `commands/tag_current_version.sh` (version
source it parses) and whether the launcher's `--version` builds refs on
demand or expects pre-built binaries. Confirm against the launcher
crate + the script when they're in scope, and update this doc.
