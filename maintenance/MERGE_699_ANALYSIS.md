# 20260719--monorepo-latest_699 — analysed, not merged

Branch preserved as `incoming/699` (tip `18f7e0c6`). The merge was attempted,
inspected, and **aborted deliberately** — the tree is clean.

## The good news first

**Self-contained**, unlike the simscope bundle: `git bundle verify` says
"records a complete history", so no prerequisite problem. And despite five
`filter-repo` passes on this side, **a shared ancestor survives** — `2ec8a7e4`.
The rewrites did not orphan this one, because it was built from a full clone
rather than as a delta.

```
their commits we lack :  1,231
our commits they lack :  1,766
their tree            :  7,478 files
```

## Their tree is a different, smaller state

68 `.iqm` totalling 3.3MB, and they are the **authored level models** — `arch`,
`arena`, `arena_base`, `boulder` — not the imported corpus. The engine loads
those, we purged them by glob, and they should come back. **Merging does not
reintroduce the 512MB**: their tree has no `blackstar-rs/assets/bots` and none
of the `imported_*` models.

## Why it is not merged

404 conflicts. Three groups, and only two are mechanical:

| group | count | resolution |
|---|---:|---|
| `engine_src/data/textures/*.png` | 168 | **ours** — losslessly re-encoded, pixel-identical, 14MB smaller |
| `Cargo.lock` | 13 | **theirs** — cargo regenerates them |
| their side is only the scaffolding commit | 4 | **ours** |
| **both sides have real later work** | **219** | **needs per-file judgement** |

I resolved the first 181 and stopped. The remaining 219 are files where both
histories moved — ours with the egui port work, theirs with things like
`feat: iOS-like weather sharing` and `refactor(av1tuner): unify inline…`.
Bulk-favouring either side would **silently discard real work**, and a merge
commit that quietly drops 219 files' worth of changes is worse than no merge,
because nothing afterwards reveals what went missing.

## What would make it tractable

* **A narrower bundle.** If the intent is specific features, `git format-patch`
  for those commits applies cleanly and conflicts only where it must.
* **Or per-area merges** — take `tools/`, then `apps/logscope`, then `games/`,
  each verified by its own tests, rather than 219 files in one commit.
* **Or a statement of which side is authoritative per area**, which turns 219
  judgements into a rule.

The analysis above is the expensive part and it is done; the remaining work is
mechanical once the intent is known.
