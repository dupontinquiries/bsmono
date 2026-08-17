# The history archive

Written immediately before the history rewrite of 2026-08-17, so nothing about
the old history depends on the old objects still existing.

```
commits-pre-rewrite.jsonl.gz   3,400 records, one per commit, OLD shas
commit-map.tsv                 old sha -> new sha (0000… means the commit
                               became empty and was dropped)
```

## Why both files

The rewrite changed every sha from the earliest touched commit forward, so the
archive's `sha` fields no longer resolve in this repo. `commit-map.tsv` is the
join. Without it the corpus is still readable but can never be tied back to a
tree, which is most of its value gone.

One commit was dropped: `2be36f0e`, "drop the 20 fold scene.bin the packs now
cover". With those files removed from all history it had nothing left to do.
That is correct rather than a loss, and it is why the map has a zero row.

## What a record holds

Per commit: sha, date, author, subject, body, a label derived from the diff
(feature/fix/docs/refactor/chore/merge), file and line counts, the paths
touched, and `entailed` — the operations the artifact PROVES happened, each
with the evidence supporting it. 44,508 of those across the archive.

`observed_calls` is null in every record, deliberately. The tool calls an
author actually made are not in git and were not reconstructed: a trace
generated from a finished diff walks straight to the answer, and a staged
wrong turn is one whose resolution was already known. See
`maintenance/tools/rollout.py` for the version that is defensible — predict
forward from repo state and the subject alone, then score against what the
commit really touched.

## The distribution

```
3,400 commits
  feature    2577    76%
  fix         406    12%
  docs        243     7%
  refactor    117     3%
  chore/merge  57     2%

  self-correcting   449   13%
  explains why    2,229   66%
  entailed ops   44,508
```

The 449 self-correcting commits are the scarce part: an author saying what they
got wrong, next to the diff that fixed it. Regenerate any of this with
`maintenance/tools/commit_corpus.py`.
