#!/usr/bin/env python3
"""unhosted_check.py — modules that compile fine and nothing calls.

WHY THIS EXISTS
---------------
Four independent sessions in one week shipped the same defect: logic that
was written, tested, and never reached from the running program.

  - snapclone: ~35 tested-but-unreachable entry points
  - logscope-fan: a trust gate with no caller
  - voiceterm: a launcher widget nothing drew
  - blackstar-rs: helmai and holomap, reachable only as CLI subcommands

`orphan_check` catches files nothing COMPILES. This catches the more
expensive case: files that compile, pass their tests, and are never
called — because that version looks finished. A crate with 400 green
tests and no call site reads as done in every summary anyone will write
about it.

WHAT IT REPORTS
---------------
For each `mod x;` in a crate root, whether any OTHER file in that crate
mentions `x::`. Two extra facts, because "no callers" alone is too blunt
to act on:

  * TESTED — the module has `#[test]`, so somebody meant it to work.
    Untested and uncalled is usually dead code to delete; tested and
    uncalled is usually work that stopped one step early, which is the
    interesting one.
  * SUBCOMMAND — the only mention is behind a CLI flag dispatch. That is
    a legitimate shape (this repo tests raids, physics and folding
    through headless sims) so it is reported separately rather than as a
    fault. The question it raises is "was that the intent", which a tool
    cannot answer.

WHAT IT DELIBERATELY DOES NOT DO
--------------------------------
It does not fail a build. A module can be legitimately uncalled — a
public API for downstream crates, a subcommand, a spike kept on purpose.
Turning this into a gate would produce an allowlist nobody maintains,
and the value here is a list a person reads after a merge, not another
thing to silence.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

MOD_RE = re.compile(r"^\s*(?:pub\s+)?mod\s+([a-z_][a-z0-9_]*)\s*;", re.M)
# A CLI dispatch: `if argv... == "--flag"` guarding a `name::run(...)`.
FLAGGED_RE = re.compile(r'==\s*"--[a-z-]+"')


def crate_roots(root: Path):
    for name in ("main.rs", "lib.rs"):
        for p in root.rglob(f"src/{name}"):
            if "/target/" in str(p) or "/.git/" in str(p):
                continue
            yield p


# The whole checked tree, so cross-crate callers are visible. Set by
# main(); None keeps the old single-crate behaviour.
_TREE_ROOT = None


def audit(rs: Path):
    text = rs.read_text(errors="replace")
    src = rs.parent
    mods = MOD_RE.findall(text)
    # SIBLINGS ACROSS THE WHOLE CHECKED TREE, not just this crate.
    #
    # Scanning only the `src/` beside the crate root reports every module
    # of a LIBRARY crate as uncalled, because its callers live in a
    # different crate. Run against gitmind that was 35 findings, and of
    # the first eight I checked by hand seven were false positives:
    # `core::checks` is called from `cli`, `varcache` has 25 cross-crate
    # references.
    #
    # A checker wrong about most of what it reports teaches people to
    # ignore it, which is worse than not having it — the same lesson as
    # the alias fix, one scope up.
    siblings = [p for p in src.rglob("*.rs") if p != rs]
    if _TREE_ROOT is not None:
        siblings = [
            p for p in _TREE_ROOT.rglob("*.rs")
            if p != rs and "/target/" not in str(p)
        ]
    out = []
    for m in mods:
        mod_file = src / f"{m}.rs"
        if not mod_file.exists():
            mod_file = src / m / "mod.rs"
        if not mod_file.exists():
            continue

        # Callers: any file other than the module itself using `m::`,
        # OR importing it under an alias.
        #
        # THE ALIAS CASE MATTERS AND WAS MISSED. `use crate::mail as mp;`
        # then `mp::inbox_ui(...)` never writes `mail::` anywhere, so a
        # `name::` search reports the module as uncalled. voiceterm does
        # this in all five of its surface modules, and the checker
        # confidently listed every one as dead — five false positives out
        # of five, which is worse than no checker, because a report that
        # is wrong about the app it is run on teaches people to ignore
        # the report.
        pat = re.compile(
            rf"\b{re.escape(m)}::"
            rf"|\buse\s+[\w:{{}}, ]*\b{re.escape(m)}\s+as\s+\w+"
        )
        callers = 0
        flagged_only = True
        for s in siblings + [rs]:
            if s == mod_file:
                continue
            body = s.read_text(errors="replace")
            hits = [
                ln for ln in body.splitlines()
                if pat.search(ln) and not ln.lstrip().startswith(("//", "///", "//!"))
            ]
            if not hits:
                continue
            callers += len(hits)
            # Is every hit next to a CLI flag test? Cheap heuristic: look
            # at the line and the two above it.
            lines = body.splitlines()
            for ln in hits:
                i = lines.index(ln)
                if not any(FLAGGED_RE.search(x) for x in lines[max(0, i - 3):i + 1]):
                    flagged_only = False

        tested = "#[test]" in mod_file.read_text(errors="replace")
        if callers == 0:
            out.append((m, "UNCALLED", tested, mod_file))
        elif flagged_only:
            out.append((m, "subcommand-only", tested, mod_file))
    return out


def main(argv) -> int:
    root = Path(argv[1] if len(argv) > 1 else ".")
    global _TREE_ROOT
    _TREE_ROOT = root
    findings = []
    for rs in crate_roots(root):
        for m, kind, tested, path in audit(rs):
            findings.append((kind, tested, str(path.relative_to(root)), m))

    uncalled = [f for f in findings if f[0] == "UNCALLED"]
    subcmd = [f for f in findings if f[0] == "subcommand-only"]

    print(f"unhosted_check: {len(uncalled)} uncalled, {len(subcmd)} subcommand-only")
    for kind, tested, path, m in sorted(uncalled):
        mark = "tested" if tested else "untested"
        print(f"  UNCALLED  {m:22s} [{mark}]  {path}")
    for kind, tested, path, m in sorted(subcmd):
        mark = "tested" if tested else "untested"
        print(f"  cli-only  {m:22s} [{mark}]  {path}")
    if uncalled or subcmd:
        print()
        print("  tested + uncalled is usually work that stopped one step early.")
        print("  untested + uncalled is usually dead code.")
        print("  cli-only may be intentional (headless sims are how this repo tests).")
    return 0  # never fails a build: see the module docstring


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
