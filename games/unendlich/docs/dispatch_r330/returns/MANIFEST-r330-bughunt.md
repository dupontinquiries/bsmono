# MANIFEST — r330-bughunt

**WID:** `r330-bughunt`
**Base:** `unendlich-r330b-mp-net_channels-and-dispatch-VERIFIED.zip`
**Merge slot (per packet):** `items → **bughunt** → mobs → menus → world → gpu`

---

## Summary

- **Item 1 (floating-trunk decay, §4e): FIXED.** Substantive bug. See below.
- **Item 2 (panic audit of `world_catalog`/`world_launch`): NO-OP — premise was a
  false positive.** Both files are already panic-free in non-test code; nothing to
  harden there. The one real non-test panic surface in the bughunt blast radius
  (leaf_decay's raw below-indexing) is hardened as part of Item 1. Details below.

## Files touched

- `crates/unendlich-engine/src/world/leaf_decay.rs` (edited; +1 module `use`, −1
  redundant inner `use`, 2 pure predicate fns added, handler loop reworked, 9 tests added).
  **No** new files. **No** `sim/mod.rs` / shared-scaffolding touch. **No** shader touch.
  **No** new dependencies.

## API added (all in `world::leaf_decay`)

- `pub fn block_supports_trunk(below: BlockId) -> bool`
  — a trunk is supported iff the block below is a log or `BlockId::is_tree_anchorage()`.
- `pub fn log_is_unsupported(block: BlockId, below: Option<BlockId>) -> bool`
  — `true` only for a log whose `below` is `Some(non-support)`. `None` (below-chunk
  not resident) ⇒ `false` (conservative; don't decay across an unloaded seam).

No public signatures changed. `decay_handler()`'s signature is unchanged.

## What was wrong (§4e: "logs left hanging when their support is gone")

The r320 floating-trunk pass had three defects; floating trunks never decayed:

1. **Dead code (root cause).** The handler iterates `chunk.plant_indices` and filters
   for `is_log`, but logs are never in `plant_indices` — `has_subvoxels()` is `false`
   for logs; they live in the separately-maintained `chunk.wood_indices`. The whole
   `is_log` branch was unreachable.
2. **Cross-chunk seam.** Even if reached, the support read was guarded by `ly > 0`, so
   a log on the chunk floor (whose support is in the chunk below) was skipped — the
   r320 author's own noted limitation ("logs at the chunk floor are skipped — can't
   see below", WIRING_STATUS §r320).
3. **Narrow + panic-prone check.** Only bare `Air` below counted as unsupported, read
   via raw indexing `chunk.blocks[idx - cs*cs]`, which underflows → OOB panic if the
   `ly>0` guard were ever relaxed.

## The fix

- **Reachability:** iterate `plant_indices.iter().chain(wood_indices.iter())`. The two
  sets are disjoint (subvoxel-foliage XOR wood), so no double-visit.
- **Cross-seam:** when `ly == 0`, resolve `below` from the top layer of the chunk at
  `(cx, cy-1, cz)` via `ctx.chunks.get(&below_coord)`; `None` if not resident.
- **Support semantics:** broadened from air-only to "not a log and not anchorage" via
  the pure `log_is_unsupported` predicate. Conservative direction — grounded/log-stacked
  trunks are never flagged (proven by `grounded_trunk_at_chunk_floor_survives` and the
  `trunk_on_ground/_log_is_supported` predicate tests).
- **Panic-safety (Item-2 win, folded in):** below-block now read with `.get(..).copied()`
  → `Option<BlockId>`; the underflow/OOB panic surface is gone.

## Item 2 — panic audit finding (honest report)

The brief stated both named files "contain them — confirmed." They do **not**, in
non-test code:

- `sim/world_catalog.rs`: only `unwrap/expect` match is the **doc comment** on line 13
  (which itself asserts there are none in non-test code). File is clean.
- `sim/world_launch.rs`: only match is `.expect("slot 0 occupied")` on line 167, which is
  **inside `#[cfg(test)]`** (test mod starts line 101) — permitted by SHARED_RULES §1.
  `resolve_load` already returns `Option`; `plan` is total.

Read-only sweep of the neighbourhood for the coordinator's awareness (NOT edited — out
of brief, MP-adjacent, owned elsewhere): `world_authority.rs`, `world_prober.rs` also
0 non-test `unwrap/expect`. Per SHARED_RULES §1 I did not fabricate panic sites to
"fix"; the audit's real value landed as the leaf_decay indexing hardening above.

## Verify — commands run + results

- **Standalone executed proof** (`trunk_proof.rs`, replicates the predicate +
  `is_tree_anchorage` + the below-index arithmetic verbatim, no `crate::`/glfw deps):
  ```
  rustc --edition 2021 --test trunk_proof.rs -o /tmp/tp && /tmp/tp
  → 9 passed / 0 failed
  ```
  Covers: trunk-on-air/leaves/water/flower ⇒ unsupported; trunk-on-ground/log ⇒
  supported; non-log never flagged; unknown-below conservative; same-chunk below =
  one layer down; floor layer has no same-chunk below (→ cross-chunk path); cross-seam
  `top_idx` = top layer of the chunk below.
- **Patch apply check:** `patch -p1` against a pristine base applies clean and
  reproduces the edited file byte-for-byte (`diff -q` → identical).

### NOT run here (coordinator's on-merge gate)
- `cargo test -p unendlich-engine --features "render-core egui" --lib leaf_decay`
  — the real gate for the 9 in-module tests (6 predicate + 3 handler integration).
  Could not run in-sandbox: the offline Rust **1.96** bundle (`rustup.zip`) was not
  uploaded this round, and the only toolchain obtainable from the allow-listed mirrors
  was apt **cargo/rustc 1.75**, which cannot read the project's **v4 `Cargo.lock`**
  without a lock-dance (SHARED_RULES forbids). The standalone proof is the documented
  substitute for the pure logic; the in-module handler tests need the crate and should
  be gated on merge under 1.96.

## Handoff notes (no bin wiring proposed)

- **Concurrency caveat to confirm on-device:** the `ly==0` path takes a second DashMap
  read (`ctx.chunks.get(&below_coord)`) while the outer per-chunk `entry` read is held.
  Reads are shared, and the tick handler runs in a phase where chunk edits are *queued*
  (`enqueue_edit`/`enqueue_state`) and applied later by ChunkManager — not mutated mid-
  handler — so no writer races the read. If your real driver ever mutates `chunks`
  concurrently with this handler, confirm the two-read pattern against your RwLock's
  reentrancy (parking_lot read-on-read is fine absent a queued writer). The neighbour
  `Ref` lifetime is minimal (resolved + copied out immediately).
- **Behavioural note:** flagged logs age out over `LEAF_DEATH_AGE_TICKS` (~100s calm)
  and cascade upward (each log → Air re-flags the one above next tick), exactly the
  r320 intent — now actually reachable. No new tunables; no signature changes.
- Pre-existing failures listed in SHARED_RULES were left untouched.
