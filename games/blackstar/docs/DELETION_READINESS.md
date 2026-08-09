# Are we ready to wipe the baked models? Not yet — three specific gaps

Asked whether the corpus can be deleted, keeping a few examples in a parallel
branch. Measured rather than answered from the last good number, and the answer
changed as a result.

## Gap 1 — the committed web could not rebuild the corpus (now fixed)

Before this session `catalog/parts_web.bin.gz` held **1,790 parts at 16-bit with
no delta encoding**: the old catalog-only web. The 40,701-part full-corpus web
was built in `/tmp` during an earlier round and **never committed**. Likewise
`importedcat` shipped 790 of 4,950 part types and 514 of 1,012 models.

Deleting the models against that state would have destroyed the corpus and left
a web that rebuilds half of it.

Fixed here: `catalog/parts_web_full.bin.gz`, **36.4MB**, 40,742 parts, all
annotated, 8-bit vertices with delta-varint indices — against the previous
54.7MB for the same coverage, a third smaller. `partlib_full.json` carries the
86,406 placements.

## Gap 2 — corpus-scale parity is 0.765, not 0.906

The 0.906 figure came from five models. On ten models with ≥15 placements:

```
soldierrib             0.735 0.924 0.876
zetrib                 0.821 0.890 0.830
autobotrb              0.744 0.896 0.902
autobotsoldlmb         0.804 0.906 0.918
zetaprirb              0.561 0.482 0.463   <-
zetaprimelmb           0.587 0.488 0.473   <-
mean                   0.765
```

Two models sit at 0.46–0.59 and pull the mean down by 0.14. **A five-model
sample was not a corpus measurement**, and this is the third time in this work
that extrapolating from a small sample proved optimistic in the same direction.

## Gap 3 — a "run-back branch" does not save the space

Worth stating because it is the trap this repo has already fallen into three
times: **a branch retaining examples keeps every object those examples reach.**
If the branch holds the full corpus, the bundle keeps all 512MB regardless of
what `main` contains. Only objects reachable from *no* ref are dropped, and only
after a history rewrite plus `gc --prune=now`.

So the demonstration branch must contain **only the handful of example models**,
built as a fresh orphan commit — not a snapshot of the current tree.

## What would make it ready

1. **Raise corpus-scale parity**, or accept 0.765 knowingly. The two failing
   models should be diagnosed first — with per-part alignment at 0.995 and
   coverage at 89–100%, a 0.46 whole-model score points at placement-to-component
   matching rather than at anything the last four rounds touched.
2. **Verify on a much larger sample** — tens of models, not eight.
3. **Render comparison by eye** on the examples that will survive, since every
   number here is silhouette IoU and nobody has looked at one.

None of that is far off. But "ready" is a claim about 1,012 models, and the
evidence currently covers eight.
