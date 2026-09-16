#!/usr/bin/env python3
"""stale_log_check.py — is this test log actually from this tree?

A crate that fails to BUILD cannot run its tests, and the harness keeps the
previous `*.test.log` rather than writing an empty one. The summary then counts
those old failures as current, and the log reads exactly like a fresh one: same
format, same test names, plausible numbers.

That cost real time once. A fix landed, the next run reported the identical
failure, and the obvious conclusion — "the fix did not work" — was wrong. The
build had broken elsewhere in the same crate, the tests never ran, and the old
log came forward.

## The check

Rust panics carry `file.rs:LINE:COL`. If a log says a test panicked at line 676
and that file's line 676 is now a blank line in the middle of a struct, the log
predates the current source. This does not prove a log is fresh — code can
change without moving a panic — but a MISMATCH is conclusive, and mismatches
are what mislead.

Byte-comparing against the previous run's log is a much weaker test and was the
first thing I tried: two identical logs usually mean a crate that did not change
and whose tests pass deterministically, which is the normal case rather than a
fault.

## Use

    stale_log_check.py <logs-dir> <repo-root>

Exit 1 if any log references a line that cannot be what panicked.
"""
import os
import re
import sys

PANIC = re.compile(r"panicked at ([\w./\-]+\.rs):(\d+):\d+")


def source_line(repo: str, relpath: str, line: int):
    """The text at `line`, searching the repo for the file. None if not found."""
    # Logs carry crate-relative paths, so the same suffix can match more than
    # one file. Any match is enough to catch a gross mismatch, and reporting a
    # false positive here would be worse than missing one, so a path that
    # matches several files is skipped rather than guessed at.
    hits = []
    want = relpath.replace("\\", "/")
    for root, dirs, files in os.walk(repo):
        dirs[:] = [d for d in dirs if d not in (".git", "target", "node_modules")]
        for f in files:
            p = os.path.join(root, f)
            if p.replace("\\", "/").endswith("/" + want):
                hits.append(p)
                if len(hits) > 1:
                    return None
    if not hits:
        return None
    try:
        with open(hits[0], errors="replace") as fh:
            lines = fh.read().split("\n")
    except OSError:
        return None
    if line < 1 or line > len(lines):
        return ""  # past the end of the file: definitely stale
    return lines[line - 1]


def looks_like_a_panic_site(text: str) -> bool:
    """Could this line have produced a panic?

    Deliberately generous. The aim is to catch a log pointing at a blank line
    or a closing brace, not to model every way Rust can panic — a check that
    cried wolf would be turned off within a week.
    """
    t = text.strip()
    if not t or t in ("}", "{", "};", ")"):
        return False
    if t.startswith("//"):
        return False
    return True


def main(argv):
    if len(argv) != 3:
        print(__doc__)
        return 2
    logs, repo = argv[1], argv[2]
    stale = []
    for name in sorted(os.listdir(logs)):
        if not name.endswith(".log"):
            continue
        with open(os.path.join(logs, name), errors="replace") as fh:
            body = fh.read()
        seen = set()
        for m in PANIC.finditer(body):
            key = (m.group(1), int(m.group(2)))
            if key in seen:
                continue
            seen.add(key)
            text = source_line(repo, key[0], key[1])
            if text is None:
                continue  # unknown or ambiguous file — say nothing
            if not looks_like_a_panic_site(text):
                stale.append((name, key[0], key[1], text.strip()[:48]))
    if not stale:
        print("no log references an impossible panic site")
        return 0
    print(f"{len(stale)} panic site(s) cannot be what the log says — these logs predate the tree:\n")
    for name, f, line, text in stale:
        print(f"  {name}: {f}:{line} is now {text!r}")
    print("\nA crate that failed to BUILD keeps its previous test log. Check the build")
    print("output for that crate before believing any failure reported here.")
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
