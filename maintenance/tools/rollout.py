#!/usr/bin/env python3
"""rollout.py — predict the tool calls forward, then score against what happened.

The objection to reconstructing tool calls from a finished diff is that the
diff is the answer. A trace generated from it walks straight to the result, and
any wrong turn in it is one whose resolution was already known — hindsight
wearing a process's clothes.

This removes the answer from the input. The prompt is what an author could see
BEFORE the commit existed:

    * the repo state at the parent (paths, sizes — not contents)
    * the commit subject, as the goal
    * nothing else: no diff, no body, no file list of what changed

A prediction made from that is a forward rollout. It can be wrong, and being
wrong is informative, which is the property the hindsight version lacks.

SCORING
-------
The commit's `entailed` ops (see commit_corpus.py) are the answer key — they
are what the artifact proves happened. A prediction is scored on which paths it
touched, not on exact call syntax, because two ways of writing the same edit
are the same decision.

    precision  of the paths predicted, how many were really touched
    recall     of the paths really touched, how many were predicted

Both matter and they fail differently: low recall means the model did not know
where the work lived; low precision means it guessed at files that were never
involved. A single score would hide which.

PROVENANCE, NON-NEGOTIABLE
--------------------------
Predictions are written to their own file with `synthetic: true`, the model
that produced them, and exactly which inputs it saw. They are never merged into
the `entailed` field, which holds only what the artifact proves. A consumer
must always be able to tell a measurement from a generation, and the moment the
two share a field that is lost for good.
"""

import argparse
import json
import subprocess
import sys


def run(args):
    return subprocess.run(["git"] + args, capture_output=True, text=True,
                          check=False).stdout


def parent_state(sha, limit=400):
    """Paths and sizes at the commit's parent — the repo as it was.

    Sizes, not contents: a prompt carrying file bodies would be enormous, and
    the question is "where would you look and what would you change", which
    paths answer.
    """
    out = run(["ls-tree", "-r", "-l", f"{sha}^"])
    rows = []
    for line in out.splitlines():
        parts = line.split(None, 4)
        if len(parts) == 5:
            rows.append({"path": parts[4], "bytes": int(parts[3])
                         if parts[3].isdigit() else 0})
    # Sorted by PATH, not size, and truncated after any area filter — an
    # earlier version did both backwards, so a size-sorted top-400 buried
    # every small .rs module under changelogs and generated QML, and the
    # --area filter then ran on a list the relevant files had already fallen
    # out of. The prompt looked reasonable and contained nothing useful.
    rows.sort(key=lambda r: r["path"])
    return rows


def subject_of(sha):
    return run(["log", "-1", "--format=%s", sha]).strip()


def touched(sha):
    """Paths the commit really changed — the answer key."""
    out = run(["show", "--numstat", "--format=", sha])
    paths = []
    for line in out.splitlines():
        cols = line.split("\t")
        if len(cols) == 3:
            paths.append(cols[2])
    return paths


def make_prompt(sha, area=None):
    """The blind input. Deliberately excludes the diff, the body and the
    changed-file list."""
    state = parent_state(sha)
    if area:
        state = [r for r in state if r["path"].startswith(area)]
    state = state[:400]
    return {
        "sha": sha,
        "goal": subject_of(sha),
        "repo_state_at_parent": state,
        "inputs_withheld": ["diff", "commit_body", "changed_files"],
    }


def score(prediction_paths, actual_paths):
    """Path-level precision and recall.

    Directories count as near misses rather than hits: predicting the right
    module but the wrong file inside it is real partial knowledge, and
    collapsing it to zero would make the metric harsher than the task.
    """
    pred, act = set(prediction_paths), set(actual_paths)
    hit = pred & act
    pred_dirs = {p.rsplit("/", 1)[0] for p in pred}
    act_dirs = {p.rsplit("/", 1)[0] for p in act}
    return {
        "predicted": len(pred),
        "actual": len(act),
        "exact": len(hit),
        "precision": round(len(hit) / len(pred), 3) if pred else 0.0,
        "recall": round(len(hit) / len(act), 3) if act else 0.0,
        "dir_recall": round(len(pred_dirs & act_dirs) / len(act_dirs), 3)
        if act_dirs else 0.0,
    }


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("prompt", help="emit the blind input for a commit")
    p.add_argument("sha")
    p.add_argument("--area", help="restrict the state listing to a path prefix")

    s = sub.add_parser("score", help="score a predictions file")
    s.add_argument("predictions", help="jsonl: {sha, paths:[...], calls:[...]}")

    a = ap.parse_args()

    if a.cmd == "prompt":
        print(json.dumps(make_prompt(a.sha, a.area), indent=2))
        return 0

    rows = [json.loads(l) for l in open(a.predictions) if l.strip()]
    tot = {"exact": 0, "predicted": 0, "actual": 0}
    for r in rows:
        sc = score(r.get("paths", []), touched(r["sha"]))
        r["score"] = sc
        for k in tot:
            tot[k] += sc[k]
        print(f"{r['sha'][:9]}  P {sc['precision']:.2f}  R {sc['recall']:.2f}  "
              f"dirR {sc['dir_recall']:.2f}  ({sc['exact']}/{sc['actual']} paths)")
    if rows:
        p_ = tot["exact"] / tot["predicted"] if tot["predicted"] else 0
        r_ = tot["exact"] / tot["actual"] if tot["actual"] else 0
        print(f"\noverall  precision {p_:.2f}  recall {r_:.2f}  over {len(rows)} commits")
        print("\nA rollout that scores well is evidence the goal was inferable from")
        print("the repo state. One that scores badly is not noise — it marks a")
        print("commit whose reasoning came from somewhere the repo does not hold.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
