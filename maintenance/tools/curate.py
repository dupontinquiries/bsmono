#!/usr/bin/env python3
"""curate.py — a feature history whose checklists are made of real failures.

Two audiences pull in opposite directions. A person or an agent reading this
tree wants features: what was built, in what order, why. A training corpus
wants the mess, because the scarce signal is a wrong assumption and the
correction that followed — and a squashed feature commit deletes exactly that.

This resolves it by re-expressing rather than discarding. Commits are grouped
into features; the self-correcting commits inside each group are mined for what
went wrong; and the curated message states the feature followed by:

    Checks that apply when doing <this kind of work>:
      - <the thing that actually went wrong, as a precondition>   [sha]

The narrative becomes a checklist. "I assumed the packs were on this branch
and they were not" becomes "confirm the superseding artifact is present on THIS
branch, per-path". Same information, expressed as what a more careful version
of the work would have checked first.

## Every check cites its commit

Non-negotiable, and the reason this is not folklore. A check whose origin
cannot be inspected is an unfalsifiable rule that accumulates forever, and
future readers cannot tell a hard-won constraint from a superstition. The sha
is the evidence, and `git show` is how you audit it.

## What is extracted vs. what is written

The tool extracts EVIDENCE: which commits self-corrected, and the sentences in
them that describe the failure. It does not invent the lesson. Turning a
paragraph into one imperative line is a rewrite of text that exists, performed
by whoever runs this — and the source sha travels with it so the rewrite can
always be checked against the original.

That distinction is the same one `rollout.py` and the `entailed` fields draw:
generated content is allowed, and it is labelled and traceable to what it came
from.
"""

import argparse
import json
import re
import subprocess
import sys
from collections import OrderedDict

# Sentences in a commit body that describe a failure the author hit. Matched
# on the phrasings that actually recur in this history rather than a general
# sentiment list — a loose matcher pulls in ordinary prose and buries the
# signal it exists to find.
FAILURE_CUE = re.compile(
    r"\b(?:i was wrong|got wrong|my (?:own )?(?:mistake|error|bug)|"
    r"caught (?:it|this|by)|first (?:attempt|version)|had assumed|i assumed|"
    r"turned out|the opposite|silently|never noticed|"
    r"did not notice|found by running|not by reading)\b",
    re.I,
)

# Sentence boundary: a period followed by whitespace and a capital or newline.
# NOT a bare period — the first version split on any `.` and produced
# fragments cut mid-number ("through StarClass::M's generic bucket (0.") and
# mid-identifier ("rs's own orbit()"). Decimals, `foo.rs`, `::` paths and
# abbreviations are all periods that do not end a sentence, and a checklist
# made of truncated clauses is worse than no checklist: it reads like a rule
# and cannot be acted on.
SENT_END = re.compile(r"(?<=[a-z\)\]\"'])\.(?=\s+[A-Z]|\s*$|\n)")


def sentences(body):
    """Split a commit body into sentences, tolerantly."""
    text = " ".join(body.split())
    out, start = [], 0
    for m in SENT_END.finditer(text):
        out.append(text[start:m.end()].strip())
        start = m.end()
    tail = text[start:].strip()
    if tail:
        out.append(tail)
    return out


def run(args):
    return subprocess.run(["git"] + args, capture_output=True, text=True).stdout


def top_area(path):
    """The feature bucket a path belongs to.

    Two levels: `apps/voiceterm`, `games/blackstar-rs`, `maintenance`. Deeper
    fragments a feature across modules; shallower merges unrelated apps.
    """
    parts = path.split("/")
    if parts[0] in ("apps", "games", "shared", "system", "tools") and len(parts) > 1:
        return f"{parts[0]}/{parts[1]}"
    return parts[0] if parts else "root"


def commits(rev_range):
    sep, rec = "\x1e", "\x1d"
    fmt = sep.join(["%H", "%aI", "%s", "%b"]) + rec
    for chunk in run(["log", "--reverse", f"--format={fmt}", rev_range]).split(rec):
        chunk = chunk.strip("\n")
        if not chunk:
            continue
        p = chunk.split(sep)
        if len(p) < 4:
            continue
        sha, date, subject, body = p[:4]
        files = [
            l.split("\t")[2]
            for l in run(["show", "--numstat", "--format=", sha]).splitlines()
            if len(l.split("\t")) == 3
        ]
        yield {"sha": sha, "date": date, "subject": subject,
               "body": body.strip(), "files": files}


def area_of(c):
    """The dominant area of a commit — most-touched bucket wins."""
    counts = {}
    for f in c["files"]:
        a = top_area(f)
        counts[a] = counts.get(a, 0) + 1
    return max(counts, key=counts.get) if counts else "root"


def group(cs):
    """Consecutive commits in the same area form one feature.

    Contiguity matters: the same area revisited a week later is a different
    piece of work, and merging them would produce a checklist spanning two
    unrelated problems.
    """
    groups, cur, cur_area = [], [], None
    for c in cs:
        a = area_of(c)
        if a != cur_area and cur:
            groups.append((cur_area, cur))
            cur = []
        cur_area = a
        cur.append(c)
    if cur:
        groups.append((cur_area, cur))
    return groups


def failures(c):
    """Sentences in one commit that describe something that went wrong."""
    out = []
    for s in sentences(c["body"]):
        if not FAILURE_CUE.search(s):
            continue
        if 40 < len(s) < 400:
            out.append(s)
    return out


def render(area, cs):
    """One curated feature entry."""
    checks = []
    for c in cs:
        for f in failures(c):
            checks.append((f, c["sha"][:9]))
    lines = [f"{area}: {cs[0]['subject'].split(':', 1)[-1].strip()}"]
    if len(cs) > 1:
        lines[0] += f" (+{len(cs) - 1} more)"
    lines += ["", "Commits:"]
    for c in cs:
        lines.append(f"  {c['sha'][:9]}  {c['subject']}")
    if checks:
        lines += ["", f"Checks that apply when working on {area}:"]
        seen = set()
        for text, sha in checks:
            key = text.lower()[:60]
            if key in seen:
                continue
            seen.add(key)
            lines.append(f"  - {text}  [{sha}]")
        lines += ["",
                  "Each check is a failure that actually occurred here; the sha is the",
                  "commit it came from. Audit one with `git show <sha>` rather than",
                  "taking it on trust — a rule whose origin cannot be inspected is",
                  "indistinguishable from a superstition."]
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("range", nargs="?", default="HEAD")
    ap.add_argument("--out", help="write the curated log here")
    ap.add_argument("--jsonl", help="machine-readable form")
    a = ap.parse_args()

    cs = list(commits(a.range))
    groups = group(cs)
    blocks, records = [], []
    for area, gcs in groups:
        blocks.append(render(area, gcs))
        records.append({
            "area": area,
            "commits": [c["sha"] for c in gcs],
            "subject": gcs[0]["subject"],
            "checks": [{"text": t, "from": s} for c in gcs for t, s in
                       [(f, c["sha"]) for f in failures(c)]],
        })

    text = "\n\n" + ("\n\n" + "-" * 72 + "\n\n").join(blocks) + "\n"
    if a.out:
        open(a.out, "w").write(text)
    else:
        sys.stdout.write(text)
    if a.jsonl:
        with open(a.jsonl, "w") as f:
            for r in records:
                f.write(json.dumps(r) + "\n")

    nchecks = sum(len(r["checks"]) for r in records)
    print(f"{len(cs)} commits -> {len(groups)} features, {nchecks} checks extracted",
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
