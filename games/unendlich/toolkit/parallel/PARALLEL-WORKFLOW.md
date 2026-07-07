# Parallel Claude development — proven workflow

The goal: run several Claude sessions at once (rendering, items, mobs, worldgen…)
and merge their results into one product **without clobbering each other**. The
clobbering you hit before came from full-tree snapshot zips overwriting the
tree. This workflow avoids that by shipping *only* patch-series bricks and
merging them mechanically, with collisions surfaced before they bite.

It is deliberately low-ceremony — three rules and two tools.

## The three rules

1. **One lane per session.** Each session owns a set of paths (see
   `lanes.conf`). It edits within its lane; touching another lane's files is
   allowed but must be a conscious choice (the checker warns). Declared lanes
   today: `rendering`, `items`, `mobs`, `worldgen`, plus `shared`
   (toolkit/docs/manifests — keep edits there additive).

2. **Ship bricks, never full-tree zips, for merging.** A brick is a
   `git format-patch` series in a zip (the `apply_from_zip_v2` layout). Patches
   3-way-merge; full-tree zips overwrite. The `*-FULL.zip` snapshots are fine as
   a *reference / single-session restore*, but **do not** apply a full snapshot
   from one session on top of another's work — that's the clobber. Cross-session
   integration is always brick → `merge_bricks`.

3. **Every feature is env-gated and starts from a known base.** Each brick’s
   `MANIFEST.env` records `base=<sha>`. Features hide behind an env flag
   (`UNENDLICH_POSTFX`, `UNENDLICH_BLOOM`, …) so a half-finished lane never
   changes default behaviour and A/B is one toggle. This is what lets sessions
   ship mid-arc without destabilizing the merged product.

## The two tools

- **`lane_check.py`** — before you ship a brick, confirm it stayed in lane:
  ```
  python3 toolkit/parallel/lane_check.py --repo . --lane rendering --staged
  python3 toolkit/parallel/lane_check.py --repo . --patches /path/patches/*.patch
  ```
  Flags out-of-lane, contested (owned by >1 lane), and unowned files.

- **`merge_bricks.sh`** — assemble N sessions' bricks into one branch:
  ```
  toolkit/parallel/merge_bricks.sh --base v0.24.103 --out merge/nightly \
      --snapshot rendering.zip items.zip mobs.zip
  ```
  It (1) reports file + lane OVERLAPS between bricks up front, (2) applies them
  in order via `git am --3way` in a throwaway worktree (your tree is untouched),
  and (3) stops on the first conflict naming the brick + files. List the most
  foundational session first. On success you get a merged branch (and, with
  `--snapshot`, a combined full zip) — that is your "final product" for the
  cycle.

## The cycle (per integration round)

1. Each session works its lane, runs `bash toolkit/harness/run.sh` (static +
   cargo + GLSL compile), and ships a brick + ran `lane_check --lane <its lane>`.
2. You collect the bricks and run `merge_bricks --base <last good> …`.
3. No overlaps → merged branch builds → it becomes the next `base` for all
   sessions (tell each session the new base sha). Overlaps/conflicts → the
   report tells you which two lanes collided and on which file; resolve by
   splitting the file across lanes, reordering, or asking one session to rebase.
4. Tag the merged result; repeat.

Because lanes are disjoint by design, most rounds have **zero** overlaps and the
merge is fully mechanical — that is the speedup. The only real coordination cost
is the handful of contested files (see `lanes.conf`), of which
`render/world_renderer.rs` is the worst: every "draw a new thing" feature reaches
into it. Keep edits there tiny and localized, or front a small registration hook
so lanes append instead of editing the same block.

---

## Worked example: kicking off the **item-advancement** thread

Use this as the opening brief for a fresh Claude session so its output merges
cleanly. (Fill the specifics from the actual `items/` code — this is the process
scaffold, not a spec of mechanics that don't exist yet.)

**Lane:** `items` — owns `crates/unendlich-engine/src/items/` (`crafting.rs`,
`inventory.rs`, `loot.rs`, `drops.rs`, plus `mod.rs`) and the item-side sim files
`sim/item_entity.rs`, `sim/skills.rs`, `sim/armor.rs`. Stay out of `render/`,
`world/` generators, and `sim/mob.rs` (other lanes).

**Base:** whatever the current merged tag is (e.g. `v0.24.104`). Record it in
every brick’s `MANIFEST.env`.

**Standing constraints to hand the session:**
- Ship patch-series bricks (the `apply_from_zip_v2` layout) + a full snapshot, as
  in the rendering thread. Run `lane_check.py --lane items` before shipping.
- Gate each new system behind an env flag (e.g. `UNENDLICH_ITEM_TIERS`) so it’s
  default-off and A/B-able, exactly like the post-FX/bloom flags.
- Add a CPU-side unit test for any new rule (mirror how `world/water_waves.rs`
  has dep-free tests the harness runs) so correctness is provable without a
  full game run — the same "prove it statically, the user only A/Bs" discipline.
- Keep a `docs/claude-notes/` readme per increment with an A/B checklist.

**A roadmap shape for "advancement"** (confirm against the real code first):
1. *Read-only pass:* the session inventories what `items/` already models
   (item ids, stacks, crafting graph, loot tables) and writes a one-page map —
   no edits. This is the cheap step that prevents wrong assumptions.
2. *Tiers / progression data:* introduce an item-tier / unlock model as data +
   pure functions with tests, gated off. No UI, no render.
3. *Crafting/loot wiring:* connect tiers into `crafting.rs`/`loot.rs` behind the
   flag.
4. *HUD/UI:* surface progression in `items/debug_ui.rs` (it already exists) —
   still items-lane; do NOT reach into the renderer.
5. Only if it needs to *draw* something new in the world does it coordinate with
   the rendering lane (a contested touch of `world_renderer.rs`), and that’s a
   deliberate, announced overlap.

This keeps the whole item thread inside its lane, so its bricks drop into
`merge_bricks` with zero overlap against the rendering thread.
