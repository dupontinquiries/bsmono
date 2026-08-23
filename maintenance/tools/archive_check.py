#!/usr/bin/env python3
"""archive_check.py — every file in maintenance/archive/ carries its provenance.

WHY

An archive without provenance is a junk drawer, and a junk drawer is worse than
a deletion: git at least tells you when and by whom a file went away, while an
undated copy sitting in a directory tells you nothing and looks maintained.

Five fields, and the validation is deliberately more than "the line exists" —
the failure mode this guards against is a header written to satisfy a checker.

  ARCHIVED         one line: what the thing is
  ORIGIN           the path it was removed from (must look like a real path)
  REMOVED          <sha> <date>  — both, and the sha must resolve
  SUPERSEDED-BY    what to read instead, or the literal NOTHING
  LAST-KNOWN-GOOD  what was CONFIRMED, when, and by what — or NEVER CONFIRMED

LAST-KNOWN-GOOD is the field that earns the directory and the easiest to fudge.
"14 tests passed at <sha>, 2026-08-22" is a claim with a date and a means.
"worked fine" is not. So it must contain either a four-digit year or the
literal `NEVER CONFIRMED`, and it may not be one of the vague phrases below —
because a reviver's first question is always "was this ever actually run", and
a header that cannot answer it has failed at its only job.
"""
import pathlib, re, subprocess, sys

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/home/claude/repo")
ARCHIVE = ROOT / "maintenance" / "archive"

FIELDS = ["ARCHIVED", "ORIGIN", "REMOVED", "SUPERSEDED-BY", "LAST-KNOWN-GOOD"]
VAGUE = ("worked fine", "should work", "probably", "seemed to", "unknown", "n/a", "tbd")


def header_of(path: pathlib.Path) -> dict:
    """Field -> value, reading only the leading comment block."""
    out, current = {}, None
    for line in path.read_text(errors="replace").splitlines():
        if not line.startswith("//"):
            break
        body = line[2:].strip()
        m = re.match(r"^([A-Z][A-Z-]+):\s*(.*)$", body)
        if m and m.group(1) in FIELDS:
            current = m.group(1)
            out[current] = m.group(2)
        elif current and body:
            out[current] += " " + body
    return out


def sha_exists(sha: str) -> bool:
    return subprocess.run(["git", "-C", str(ROOT), "cat-file", "-e", sha],
                          capture_output=True).returncode == 0


def main() -> int:
    if not ARCHIVE.is_dir():
        print("no maintenance/archive/ — nothing to check")
        return 0

    files = sorted(p for p in ARCHIVE.iterdir()
                   if p.is_file() and p.name != "README.md")
    problems = []

    for p in files:
        h = header_of(p)
        for f in FIELDS:
            if f not in h or not h[f].strip():
                problems.append(f"{p.name}: missing {f}")

        origin = h.get("ORIGIN", "")
        if origin and "/" not in origin:
            problems.append(f"{p.name}: ORIGIN {origin!r} is not a path")

        removed = h.get("REMOVED", "")
        m = re.match(r"^([0-9a-f]{7,40})\s+(\d{4}-\d{2}-\d{2})", removed)
        if not m:
            problems.append(f"{p.name}: REMOVED must be '<sha> <YYYY-MM-DD>', got {removed!r}")
        elif not sha_exists(m.group(1)):
            problems.append(f"{p.name}: REMOVED sha {m.group(1)} does not resolve")

        lkg = h.get("LAST-KNOWN-GOOD", "")
        low = lkg.lower()
        if "NEVER CONFIRMED" not in lkg and not re.search(r"\b\d{4}\b", lkg):
            problems.append(
                f"{p.name}: LAST-KNOWN-GOOD needs a year or the literal 'NEVER CONFIRMED' — "
                f"a reviver's first question is whether this was ever run. Got: {lkg[:60]!r}")
        for v in VAGUE:
            if low.startswith(v) or f" {v} " in f" {low} ":
                problems.append(f"{p.name}: LAST-KNOWN-GOOD says {v!r}; say what was confirmed and when")

    print(f"checked {len(files)} archived file(s)")
    for msg in problems:
        print(f"  {msg}")
    if not problems:
        print("all carry complete provenance")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
