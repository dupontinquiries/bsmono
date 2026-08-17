# The history archive

Written immediately before the history rewrite of 2026-08-17, so nothing about
the old history depends on the old objects still existing.

```
commits-pre-rewrite.jsonl.gz   3,400 records, one per commit, OLD shas
commit-multimap.tsv            original -> every later sha  <- USE THIS
commit-map.tsv                 rewrite 1 only; superseded, kept as evidence
```

`commit-multimap.tsv` has four columns: `original`, `gen1_paths_removed`,
`current`, `provenance`. Resolve an archive record against the tree by joining
`sha` to `original` and reading `current`.

## Why a multimap, and what went wrong

There have been two rewrites: paths removed, then the superseded v3 pack blobs
removed by content. `commit-map.tsv` only covers the first, so after the second
its "new" column names commits that no longer exist — the archive pointed at a
generation that had itself been rewritten away.

The chain should have been composed from both runs' maps. It could not be:
`.git/filter-repo/commit-map` is **overwritten by the next run**, and filter-repo
was run more than twice here (one of those runs was a no-op caused by a
malformed blob callback, which still rewrote). The intermediate map was never
copied out and is gone. Only 291 of 3,428 shas overlap between the two maps
that survive, which is how the break was found — not by the tooling reporting
an error.

**Copy `.git/filter-repo/commit-map` out immediately after every run.** That is
the whole lesson.

## How `current` was recovered

By content, not by derivation. A rewrite that removes blobs leaves subjects and
author dates untouched, so archive records were matched to current commits on
`(author_date, subject)`.

That key is not unique here — about 1,165 keys are held by two commits each,
genuine duplicates committed in the same second. Order breaks the tie: both
lists are in commit order, so the i-th occurrence of a key maps to the i-th
occurrence. Exact whenever the rewrite preserved order, which a blob filter
does.

3,395 of 3,400 resolved; 30 of 30 sampled had matching subjects. The five that
did not are all explicable and are marked `dropped` or `unmatched` rather than
guessed:

* `2be36f0e` — deleted the fold `scene.bin`; rewrite 1 removed those files from
  all history, so the commit had nothing left to do.
* `16047d2314`, `f934144e0c` — content was entirely v3 pack blobs, emptied by
  rewrite 2.
* two `paperbound` commits sharing a subject and second, where one copy no
  longer exists.

Regenerate with `maintenance/tools/commit_multimap.py`.

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
