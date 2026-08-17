#!/usr/bin/env python3
"""commit_multimap.py — chain filter-repo's per-run maps into one lineage table.

Each history rewrite emits `.git/filter-repo/commit-map`: two columns, the sha
before that run and the sha after it. Useful once. After a second rewrite it is
actively misleading, because the "new" column names commits that no longer
exist — the archive in `maintenance/history/` points at a generation that was
itself rewritten away.

This composes them. Given the maps in order, oldest run first, it emits one row
per ORIGINAL commit carrying every sha that commit has ever had:

    original <TAB> gen1 <TAB> gen2 <TAB> ... <TAB> current

so a record in `commits-pre-rewrite.jsonl` can still be resolved against the
tree, however many rewrites later.

## Dropped commits

A rewrite can delete a commit outright — filter-repo writes all-zeros for one
that became empty. That is not a gap to be papered over: the commit existed,
its message and diff are in the archive, and the honest row says it stopped
existing at a named generation rather than silently ending the lineage. Those
rows keep the zeros and everything after them is empty.

## What this cannot recover

A rewrite whose map was not kept. `.git/filter-repo/commit-map` is overwritten
by the next run, so a map that was never copied out is gone, and the chain
across that gap cannot be reconstructed from the repo — the shas on either side
share no derivation anything can compute. Copy the map out after every run;
this script is the reason.
"""

import argparse
import sys

ZERO = "0" * 40


def load(path):
    """One filter-repo map: {old: new}. Skips the header line if present."""
    m = {}
    with open(path) as f:
        for line in f:
            parts = line.split()
            if len(parts) != 2 or parts[0] == "old":
                continue
            m[parts[0]] = parts[1]
    return m


def compose(maps):
    """Walk each original sha forward through every generation.

    Keyed on the FIRST map's originals, because those are the shas the archive
    holds. A commit introduced by a later rewrite has no original and is
    reported separately rather than being given a fake one.
    """
    rows = []
    for orig in maps[0]:
        lineage = [orig]
        cur = orig
        for m in maps:
            if cur == ZERO or cur is None:
                lineage.append("")
                continue
            nxt = m.get(cur)
            if nxt is None:
                # Unchanged by this run: filter-repo omits identity rows in
                # some versions, so absence means "carried through", not lost.
                lineage.append(cur)
                continue
            lineage.append(nxt)
            cur = nxt
        rows.append(lineage)
    return rows


def recover_by_content(archive_pairs, current_pairs):
    """Rebuild a mapping when the intermediate map was lost.

    `.git/filter-repo/commit-map` is overwritten by the next run, so running
    filter-repo twice without copying it out destroys the chain — which is
    exactly what happened here. The shas on either side share no computable
    derivation, but the CONTENT survived: a rewrite that only removes blobs
    leaves subjects and author dates untouched.

    (date, subject) is not unique — this history has ~1,165 keys held by two
    commits each, real duplicates committed in the same second. Order breaks
    the tie: both lists are in the same commit order, so the i-th occurrence
    of a key maps to the i-th occurrence. That is exact whenever the rewrite
    preserved order, which a blob filter does.

    Returns (mapping, unmatched) so the caller can report the gap rather than
    silently returning a partial answer.
    """
    from collections import defaultdict
    buckets = defaultdict(list)
    for sha, key in current_pairs:
        buckets[key].append(sha)
    cursor = defaultdict(int)
    mapping, unmatched = {}, []
    for sha, key in archive_pairs:
        i = cursor[key]
        if i < len(buckets.get(key, ())):
            mapping[sha] = buckets[key][i]
            cursor[key] = i + 1
        else:
            unmatched.append(sha)
    return mapping, unmatched


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("maps", nargs="+",
                    help="filter-repo commit-map files, OLDEST RUN FIRST")
    ap.add_argument("--out", help="write the multimap TSV here")
    a = ap.parse_args()

    maps = [load(p) for p in a.maps]
    rows = compose(maps)
    gens = len(maps)

    header = ["original"] + [f"gen{i+1}" for i in range(gens)]
    lines = ["\t".join(header)]
    dropped = 0
    for r in rows:
        if r[-1] == "" or r[-1] == ZERO:
            dropped += 1
        lines.append("\t".join(r))

    text = "\n".join(lines) + "\n"
    if a.out:
        open(a.out, "w").write(text)
    else:
        sys.stdout.write(text)

    print(f"{len(rows)} commits across {gens} rewrite(s)", file=sys.stderr)
    print(f"{dropped} no longer exist (dropped by a rewrite)", file=sys.stderr)
    live = len(rows) - dropped
    print(f"{live} resolve to a current sha", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
