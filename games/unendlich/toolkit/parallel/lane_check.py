#!/usr/bin/env python3
"""lane_check.py — classify changed files into parallel-thread lanes.

Reads the lane map (toolkit/parallel/lanes.conf) and a set of changed file
paths, then reports which lane owns each file. Use it to catch the two things
that break parallel Claude sessions:

  * out-of-lane edits  — a session's brick touches files another lane owns
    (the usual cause of cross-session clobbering), and
  * contested / unowned files — paths owned by two lanes, or by none.

Inputs (pick one source of file paths):
  --staged                 git diff --cached --name-only   (the staged tree)
  --range A..B             git diff --name-only A..B        (a commit range)
  --patches f1.patch ...   parse the +++ b/<path> lines out of patch files
  --files -                read newline-separated paths from stdin

Options:
  --lane <name>   declare the active lane; files NOT owned by it (and not
                  `shared`) are flagged out-of-lane.
  --repo <dir>    repo root (default: cwd). Used for git modes + to find lanes.conf.
  --lanes <file>  lane map (default: <repo>/toolkit/parallel/lanes.conf).
  --strict        exit non-zero if any out-of-lane / contested / unowned file.

Exit code is 0 on a clean classification (or any issues without --strict),
non-zero on usage error or, with --strict, on any flagged file.
"""
import argparse
import fnmatch
import os
import subprocess
import sys


def parse_lanes(path):
    lanes = {}
    with open(path) as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line or "=" not in line:
                continue
            name, rest = line.split("=", 1)
            pats = [p.strip() for p in rest.split(",") if p.strip()]
            if name.strip():
                lanes[name.strip()] = pats
    return lanes


def owners(path, lanes):
    """Return the set of lane names that own `path` (prefix or fnmatch)."""
    hit = set()
    for lane, pats in lanes.items():
        for p in pats:
            if any(ch in p for ch in "*?[]"):
                if fnmatch.fnmatch(path, p) or fnmatch.fnmatch(path, p + "/*"):
                    hit.add(lane)
                    break
            else:
                # prefix match: "src/render/" matches "src/render/x.rs";
                # an exact file path matches itself.
                if path == p or path.startswith(p):
                    hit.add(lane)
                    break
    return hit


def files_from_patches(patch_paths):
    files = set()
    for pp in patch_paths:
        with open(pp, errors="replace") as fh:
            for line in fh:
                if line.startswith("+++ b/"):
                    files.add(line[6:].strip())
                elif line.startswith("--- a/"):
                    files.add(line[6:].strip())
    files.discard("/dev/null")
    return sorted(files)


def git_files(repo, args):
    out = subprocess.run(
        ["git", "-C", repo, "diff", "--name-only", *args],
        capture_output=True, text=True,
    )
    if out.returncode != 0:
        sys.exit(f"git diff failed: {out.stderr.strip()}")
    return sorted(f for f in out.stdout.splitlines() if f.strip())


def main():
    ap = argparse.ArgumentParser(description="classify changed files into lanes")
    ap.add_argument("--repo", default=".")
    ap.add_argument("--lanes")
    ap.add_argument("--lane", help="active lane to check edits stay within")
    ap.add_argument("--staged", action="store_true")
    ap.add_argument("--range")
    ap.add_argument("--patches", nargs="+")
    ap.add_argument("--files", help="'-' to read paths from stdin")
    ap.add_argument("--strict", action="store_true")
    a = ap.parse_args()

    lanes_path = a.lanes or os.path.join(a.repo, "toolkit", "parallel", "lanes.conf")
    if not os.path.isfile(lanes_path):
        sys.exit(f"lane map not found: {lanes_path}")
    lanes = parse_lanes(lanes_path)
    if a.lane and a.lane not in lanes:
        sys.exit(f"--lane '{a.lane}' is not a declared lane: {', '.join(sorted(lanes))}")

    if a.patches:
        files = files_from_patches(a.patches)
    elif a.staged:
        files = git_files(a.repo, ["--cached"])
    elif a.range:
        files = git_files(a.repo, [a.range])
    elif a.files == "-":
        files = sorted(l.strip() for l in sys.stdin if l.strip())
    else:
        ap.error("need one of --staged / --range / --patches / --files -")

    if not files:
        print("no changed files.")
        return 0

    by_lane = {}
    contested, unowned, out_of_lane = [], [], []
    for f in files:
        own = owners(f, lanes)
        if not own:
            unowned.append(f)
            by_lane.setdefault("(unowned)", []).append(f)
        else:
            for o in own:
                by_lane.setdefault(o, []).append(f)
            if len(own) > 1:
                contested.append((f, sorted(own)))
        if a.lane and own and a.lane not in own and "shared" not in own:
            out_of_lane.append((f, sorted(own)))

    print(f"== lane_check: {len(files)} changed file(s) ==")
    for lane in sorted(by_lane):
        fs = by_lane[lane]
        print(f"  [{lane}] {len(fs)}")
        for f in fs:
            print(f"      {f}")

    issues = 0
    if a.lane:
        print(f"\nactive lane: {a.lane}")
    if out_of_lane:
        issues += len(out_of_lane)
        print(f"\n!! OUT OF LANE ({len(out_of_lane)}) — owned by another lane:")
        for f, own in out_of_lane:
            print(f"     {f}  -> {', '.join(own)}")
    if contested:
        print(f"\n~~ CONTESTED ({len(contested)}) — owned by >1 lane (pick an owner in lanes.conf):")
        for f, own in contested:
            print(f"     {f}  -> {', '.join(own)}")
        issues += len(contested)
    if unowned:
        print(f"\n?? UNOWNED ({len(unowned)}) — no lane claims these (add a glob to lanes.conf):")
        for f in unowned:
            print(f"     {f}")
        issues += len(unowned)

    if issues == 0:
        print("\nOK — all changes accounted for" + (f", within lane '{a.lane}'." if a.lane else "."))
    else:
        print(f"\n{issues} item(s) flagged.")
    return 1 if (a.strict and issues) else 0


if __name__ == "__main__":
    sys.exit(main())
