# The plan, with the arithmetic checked

Proposed: keep the git history, the docs, an orphan demo branch, a validator
library of one or two `.iqm` per vehicle type plus one high-res model, and drop
the rest. **The arithmetic works.** One part of the framing needs correcting,
and one measurement changes what "a few examples" can cost.

## "Keep the git history" — yes, and it is not in tension with the purge

This is the part worth being precise about, because it sounds like it should be
a trade-off and is not. `git filter-repo` **rewrites** history, it does not
truncate it: every commit, message and code change survives; only the named
paths are removed from each tree.

Verified rather than asserted — after three purges this session (catalog `.iqm`,
`apps/unendlich`, `apps/cybertron`) the repo still holds **1,775 commits**, and
24 of them still mention unendlich by name in their subjects. The reasoning is
all still there. What leaves is the blob content, which is the only thing that
was costing anything.

## The arithmetic

```
current pack             629.9 MB
minus .iqm purged       -241.9 MB     (1,133 files, all in engine_src/data)
plus examples kept        +20.0 MB    (~20 models, generous)
                         --------
projected                 408.0 MB     92 MB under budget
```

Note `.iqm` is **241.9MB packed**, not the 302MB that `engine_src/data` reports
as a directory — the 298 textures live inside the same folder and account for
most of the difference. **Purge `*.iqm` by glob, not the directory**, or the
textures go with them and the engine loses its materials.

## What the 92MB of headroom buys

A validator library can be generous. Roughly:

* **one or two `.iqm` per vehicle type** — the `placements` keys carry the type
  in their names (`tank--`, `flyer--`, `car-`, `truck-`), so the selection is
  mechanical rather than a judgement call
* **one high-res model kept whole** — the largest in the corpus is
  `imported_xfill00_part0` at 11MB, and `scorponok` at 8.3MB; either fits
  several times over
* room to keep more if the parity work later wants a harder test set

## What must still be true before pulling the trigger

From `DELETION_READINESS.md`, unchanged by this plan:

1. Corpus-scale parity is **0.765**, measured on eight models, not the 0.906
   from five. Two models sit at 0.46–0.59 and have not been diagnosed.
2. Nobody has **looked at a render**. Every number is silhouette IoU.
3. The demo branch must be an **orphan commit holding only the examples**. A
   branch that snapshots the current tree keeps every object it reaches, and the
   512MB stays in the bundle whatever `main` says.

The plan is sound and the budget is comfortable. The gate is fidelity evidence,
not size arithmetic — and that ordering is worth keeping, because the size
argument will be just as true after the two failing models are understood.
