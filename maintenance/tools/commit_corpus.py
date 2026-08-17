#!/usr/bin/env python3
"""commit_corpus.py — turn history into labelled records, honestly.

Three consumers want different things out of this repo's history and they are
in tension:

* **A person or a Claude reading the code** wants a feature-level narrative:
  what was built, in what order, why.
* **A training corpus** wants the opposite — the messy sequence, because the
  signal is a wrong assumption, the test that caught it, and the correction.
  A squashed feature commit shows the destination and deletes the route.
* **Delta compression** wants neither near a binary blob.

So: do not choose. Keep the real history, derive the narrative, and label every
commit well enough that a consumer can filter to the slice it wants.

WHAT THIS REFUSES TO DO
-----------------------
It does not invent tool calls. The commands an author ran — the greps, the
failed builds, the test reruns — are not in git and cannot be recovered from
it. Synthesising a plausible sequence would produce fluent fiction that reads
exactly like data, and a model trained on it would learn to imitate a process
that never happened. Every record therefore carries `process: "unknown"` for
history, and the fix is to capture tool calls going forward, not to backfill
them.

CLASSIFY FROM THE DIFF, NOT THE MESSAGE
---------------------------------------
A message saying "fix" that touches only test files is a test commit. A message
saying "wire X" that adds 400 lines to a new module is a feature. Messages are
claims; the diff is evidence. Where only the message can tell us something —
whether the author was correcting themselves — the record says so, so a
consumer can weight it lower.
"""

import argparse
import json
import re
import subprocess
import sys

# ── categories ───────────────────────────────────────────────────────────────
#
# Deliberately few. A taxonomy with twenty labels is one where the boundaries
# are guesses, and a consumer filtering on a guessed boundary gets a biased
# slice without knowing it.

FEATURE = "feature"      # new capability: net new code in non-test files
FIX = "fix"              # changes existing behaviour, message says it corrects
TEST = "test"            # only test FILES changed — rare here, see below
DOCS = "docs"            # only prose changed
CHORE = "chore"          # deps, config, formatting, deletions of dead files
MERGE = "merge"          # a merge commit
REFACTOR = "refactor"    # code moved, roughly balanced add/delete, no new tests

# Measured: this label fires on ~0 of 2,100 commits, and that is not a bug in
# the pattern (verified against real paths). Rust puts tests INSIDE the source
# file under `#[cfg(test)] mod tests`, so a change to a test is a change to
# src/foo.rs and is indistinguishable, by path, from a change to the code.
#
# Left in rather than deleted because the repo also holds Python tools and
# `tests/` integration crates where it does fire, and because a consumer
# filtering for test commits should get an honest empty set rather than a
# category that quietly means something else. Separating inline test edits
# would mean diffing #[cfg(test)] regions — real work, not done here.
TEST_PAT = re.compile(r"(^|/)(tests?|testdata|fixtures)/|_test\.|test_.*\.py$")
DOC_PAT = re.compile(r"\.(md|txt|rst)$|(^|/)docs?/")
CHORE_PAT = re.compile(
    r"(^|/)(Cargo\.lock|Cargo\.toml|package\.json|pyproject\.toml|\.gitignore)$"
    r"|(^|/)packaging/|\.ya?ml$|\.toml$"
)

# Phrases where the author is correcting themselves. Message-derived and
# therefore weaker evidence than a diff — flagged as such in the record.
CORRECTION_PAT = re.compile(
    r"\b(i was wrong|got wrong|my (own )?(mistake|error|bug)|wrongly|"
    r"correct(ing|ion|s) (the|my|a)|the opposite|turned out|"
    r"caught (it|this|by)|first attempt|had assumed|assumed)\b",
    re.I,
)

# Test-run claims in a message: "106 tests pass", "1258 passed", "42 tests".
# An entailed fact, not a guess — the number was read off a run that happened.
TESTRUN_PAT = re.compile(
    r"(\d[\d,]*)\s+(?:tests?\s+(?:pass|passed|green)|passed|tests?\b)", re.I)

# A commit whose message explains WHY, not just what. The scarce signal.
REASONING_PAT = re.compile(r"\bbecause\b|\brather than\b|\binstead of\b|\bso that\b", re.I)


def run(args):
    return subprocess.run(
        ["git"] + args, capture_output=True, text=True, check=False
    ).stdout


def commits(rev_range):
    """Each commit as (sha, author, iso date, subject, body, parents)."""
    sep = "\x1e"
    fmt = sep.join(["%H", "%an", "%aI", "%s", "%b", "%P"]) + "\x1d"
    raw = run(["log", "--reverse", f"--format={fmt}", rev_range])
    for rec in raw.split("\x1d"):
        rec = rec.strip("\n")
        if not rec:
            continue
        parts = rec.split(sep)
        if len(parts) < 6:
            continue
        sha, author, date, subject, body, parents = parts[:6]
        yield {
            "sha": sha,
            "author": author,
            "date": date,
            "subject": subject,
            "body": body.strip(),
            "parents": parents.split(),
        }


def numstat(sha):
    """(added, deleted, path) per file. Binary files report as None counts."""
    out = run(["show", "--numstat", "--format=", sha])
    files = []
    for line in out.splitlines():
        cols = line.split("\t")
        if len(cols) != 3:
            continue
        a, d, path = cols
        files.append((None if a == "-" else int(a), None if d == "-" else int(d), path))
    return files


def classify(c, files):
    """Label a commit from what it touched.

    Order matters: the checks are arranged most-specific first, because a
    commit that only adds tests is a test commit even if its message sounds
    like a feature, and a merge is a merge whatever it contains.
    """
    if len(c["parents"]) > 1:
        return MERGE
    if not files:
        return CHORE

    paths = [p for _, _, p in files]
    is_test = [bool(TEST_PAT.search(p)) for p in paths]
    is_doc = [bool(DOC_PAT.search(p)) for p in paths]
    is_chore = [bool(CHORE_PAT.search(p)) for p in paths]

    if all(is_test):
        return TEST
    if all(is_doc):
        return DOCS
    if all(a or b or c_ for a, b, c_ in zip(is_test, is_doc, is_chore)):
        return CHORE

    # Code changed. Added vs deleted decides feature from refactor, and the
    # message decides fix — but only for code commits, so a docs commit
    # saying "fix" does not become one.
    added = sum(a or 0 for a, _, _ in files)
    deleted = sum(d or 0 for _, d, _ in files)
    if CORRECTION_PAT.search(c["subject"] + " " + c["body"]):
        return FIX
    # Net growth of a third or more reads as new capability; roughly balanced
    # reads as moving code around.
    if added > deleted * 1.33:
        return FEATURE
    return REFACTOR


def entailed(c, files):
    """Operations the artifact PROVES happened.

    The distinction this file turns on. A reconstruction of what an author
    plausibly typed is model output wearing a trace's clothes: generated from
    the finished diff, it walks straight to the answer, and staging a wrong
    turn means staging one whose resolution is already known. Training on that
    teaches diagnosis-as-a-straight-line, which is the opposite of what the
    self-correcting commits contain.

    These are different. A commit that adds a file entails a write. A message
    quoting "106 tests pass" entails a run that returned 106 — the number was
    read off something. Each carries the evidence that supports it, so a
    consumer can filter by how much it trusts, and nothing here is imagined.

    What is still missing is stated as missing: the greps, the failed builds,
    the reruns. Those left no trace and no amount of inference recovers them.
    """
    ops = []
    for a, d, path in files:
        if a is None and d is None:
            ops.append({"op": "write_binary", "path": path, "evidence": "diff"})
        elif d == 0 or (a and not d):
            ops.append({"op": "create_or_append", "path": path,
                        "added": a, "evidence": "diff"})
        elif a == 0:
            ops.append({"op": "delete_or_truncate", "path": path,
                        "deleted": d, "evidence": "diff"})
        else:
            ops.append({"op": "edit", "path": path, "added": a,
                        "deleted": d, "evidence": "diff"})
    # A quoted test count came from a run. The count is evidence; the command
    # that produced it is not recorded, so it is not claimed.
    for m in TESTRUN_PAT.finditer(c["subject"] + " " + c["body"]):
        n = m.group(1).replace(",", "")
        if n.isdigit() and int(n) > 0:
            ops.append({"op": "test_run", "reported": int(n),
                        "evidence": "message", "command": None})
    return ops


def record(c, files):
    paths = [p for _, _, p in files]
    added = sum(a or 0 for a, _, _ in files)
    deleted = sum(d or 0 for _, d, _ in files)
    binary = [p for a, _, p in files if a is None]
    body = c["body"]
    return {
        "sha": c["sha"],
        "date": c["date"],
        "author": c["author"],
        "subject": c["subject"],
        "body": body,
        "label": classify(c, files),
        "files": len(files),
        "added": added,
        "deleted": deleted,
        # Binary paths are called out because they are the ones that thrash
        # delta compression, and a patches-only training branch should exclude
        # them rather than carry base64 blobs.
        "binary": binary,
        "paths": paths[:40],
        # Message-derived, weaker than the label. Kept separate so a consumer
        # can decide how much to trust it.
        "self_correction": bool(CORRECTION_PAT.search(c["subject"] + " " + body)),
        "explains_why": bool(REASONING_PAT.search(body)),
        "body_lines": len(body.splitlines()) if body else 0,
        # NOT RECOVERABLE FROM GIT. Stated per record rather than omitted, so
        # a consumer cannot mistake absence for "there were none", and so a
        # later pipeline that does capture them has a field to fill.
        # Operations the artifact proves. See entailed().
        "entailed": entailed(c, files),
        # The real trace — what was actually typed, in what order, including
        # everything that failed. Not in git, not inferable from it, and
        # deliberately NOT reconstructed: a plausible trace generated from the
        # finished diff is model output that cannot be told apart from a
        # recording once it is in this field.
        "observed_calls": None,
        "process": "unknown",
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("range", nargs="?", default="HEAD",
                    help="rev range, e.g. 71163e4e..HEAD")
    ap.add_argument("--jsonl", help="write one record per line here")
    ap.add_argument("--summary", action="store_true", help="counts by label")
    a = ap.parse_args()

    recs = []
    for c in commits(a.range):
        recs.append(record(c, numstat(c["sha"])))

    if a.jsonl:
        with open(a.jsonl, "w") as f:
            for r in recs:
                f.write(json.dumps(r) + "\n")

    if a.summary or not a.jsonl:
        by = {}
        for r in recs:
            by[r["label"]] = by.get(r["label"], 0) + 1
        total = len(recs)
        print(f"{total} commits")
        for k in sorted(by, key=lambda k: -by[k]):
            print(f"  {k:<9} {by[k]:>5}  {100*by[k]/max(total,1):>4.0f}%")
        corr = sum(1 for r in recs if r["self_correction"])
        why = sum(1 for r in recs if r["explains_why"])
        binc = sum(1 for r in recs if r["binary"])
        print(f"\n  self-correcting  {corr:>5}  {100*corr/max(total,1):>4.0f}%")
        print(f"  explains why     {why:>5}  {100*why/max(total,1):>4.0f}%")
        print(f"  touches binary   {binc:>5}  {100*binc/max(total,1):>4.0f}%")
        print("\n  process (tool calls): unknown for all — not in git, not inferred")
    return 0


if __name__ == "__main__":
    sys.exit(main())
