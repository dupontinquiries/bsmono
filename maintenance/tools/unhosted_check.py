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


PUB_FN_RE = re.compile(r"\bpub\s+(?:async\s+)?fn\s+([a-z_][a-z0-9_]*)")
# Names too common to be evidence of anything.
_DUP_EXEMPT = {
    "new", "default", "run", "len", "is_empty", "get", "set", "add", "clear", "name",
    "label", "parse", "render", "fmt", "from_str", "to_string", "iter", "next", "all",
    "load", "save", "start", "stop", "update", "draw", "ui", "tick", "reset", "push",
}


def duplicate_hint(mod_file: Path, cache, self_paths):
    """Does this uncalled module define a function that ALSO exists elsewhere, live?

    The strongest triage signal there is, and it is mechanical. Of the four answers a
    hit can have — superseded, unfinished, speculative, or an API whose consumer is
    unwritten — only SUPERSEDED leaves a fingerprint a tool can find: the same job
    done under the same name somewhere that is actually called.

    Found by hand first. `kvbudget` in gpu-gemm computes kv-bytes-per-token, and so do
    three other places, two of them live; reading four files to learn that is what this
    line now saves. It is a hint, not a verdict — same name is not same behaviour, and
    the reader still has to look.
    """
    try:
        body = mod_file.read_text(errors="replace")
    except OSError:
        return None
    # COMPOUND NAMES ONLY. A bare verb is a coincidence, not evidence: the first run of this
    # flagged 51 of 75 modules, and the hints were led by `explain()` eleven times, then
    # `check()`, `report()`, `route()`, `assign()`. Two files both having a function called
    # `explain` says nothing about either. A distinctive name in this codebase is compound —
    # `kv_bytes_per_token`, `resolve_check_roots`, `parse_test_results` — and that is the shape
    # that means "the same job, done twice".
    #
    # A hint that fires on two thirds of the list is not a hint; it is the list again.
    names = {n for n in PUB_FN_RE.findall(body)
             if "_" in n and n not in _DUP_EXEMPT and len(n) > 6}
    if not names:
        return None
    for name in sorted(names):
        defn = re.compile(rf"\bfn\s+{re.escape(name)}\s*[(<]")
        call = re.compile(rf"\b{re.escape(name)}\s*\(")
        for q, (lines, code) in cache.items():
            if q in self_paths or q.suffix != ".rs":
                continue
            if not any(defn.search(ln) for _, ln in code):
                continue
            # The twin exists. Is IT called, from a third file, outside tests?
            for r, (_l2, code2) in cache.items():
                if r == q or r in self_paths or r.suffix != ".rs":
                    continue
                if any(call.search(ln) for _, ln in code2):
                    return f"{name}() also in {q.name}, which is called"
    return None


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
        # ALSO the non-Rust sources. A crate exposed over a C ABI has its
        # callers in C++, and a Rust-only scan cannot see them: snapclone
        # reported 19 uncalled modules of which `frame` alone has 109
        # references from src/*.cpp. That is a fifth false-positive class
        # after aliases and crate boundaries, and the same lesson —
        # a checker wrong about the app it is run on gets ignored.
        exts = (".rs", ".cpp", ".cc", ".h", ".hpp", ".qml")
        siblings = [
            p for p in _TREE_ROOT.rglob("*")
            if p.is_file()
            and p.suffix in exts
            and p != rs
            and "/target/" not in str(p)
        ]
    # READ EVERY SIBLING ONCE, not once per module.
    #
    # `testonly_check`'s own header records this exact lesson — "re-reading a
    # file per candidate turned a 20s run into minutes" — and this sibling tool
    # never got the fix: the loop below re-read every file in the tree for every
    # `mod` in the crate root, which is O(modules x files) full reads. Same bug,
    # same shape, one tool over.
    cache = {}
    for s_ in siblings + [rs]:
        try:
            body = s_.read_text(errors="replace")
        except OSError:
            continue
        lines = body.splitlines()
        # Comment lines can never be a caller, so drop them once rather than per
        # module. Keeping the original index lets the CLI-flag heuristic look at
        # the real neighbours.
        code = [(i, ln) for i, ln in enumerate(lines)
                if not ln.lstrip().startswith(("//", "///", "//!", "*"))]
        cache[s_] = (lines, code)

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
        # The bare NAME matters for the FFI case: C++ refers to a Rust
        # module through a header or symbol named after it
        # (`snapclone_requests_ffi.h`), never as `requests_ffi::`. Matched
        # only in non-Rust files, so a Rust file merely mentioning the
        # word in prose still does not count as a caller.
        pat = re.compile(
            rf"\b{re.escape(m)}::"
            rf"|\buse\s+[\w:{{}}, ]*\b{re.escape(m)}\s+as\s+\w+"
        )
        ffi_pat = re.compile(rf"\b{re.escape(m)}\b")
        callers = 0
        flagged_only = True
        for s in siblings + [rs]:
            if s == mod_file or s not in cache:
                continue
            lines, code = cache[s]
            use_pat = ffi_pat if s.suffix != ".rs" else pat
            # Carry the line INDEX with the hit. The previous form searched the
            # hit text back with `lines.index(ln)`, which returns the FIRST line
            # equal to that text — so two identical lines in a file had the CLI
            # heuristic inspect the wrong one's neighbours.
            hits = [(i, ln) for i, ln in code if use_pat.search(ln)]
            if not hits:
                continue
            callers += len(hits)
            # Is every hit next to a CLI flag test? Cheap heuristic: look
            # at the line and the two above it.
            for i, _ln in hits:
                if not any(FLAGGED_RE.search(x) for x in lines[max(0, i - 3):i + 1]):
                    flagged_only = False

        tested = "#[test]" in mod_file.read_text(errors="replace")
        if callers == 0:
            hint = duplicate_hint(mod_file, cache, {mod_file, rs})
            out.append((m, "UNCALLED", tested, mod_file, hint))
        elif flagged_only:
            out.append((m, "subcommand-only", tested, mod_file, None))
    return out


def main(argv) -> int:
    root = Path(argv[1] if len(argv) > 1 else ".")
    global _TREE_ROOT
    _TREE_ROOT = root
    findings = []
    for rs in crate_roots(root):
        for m, kind, tested, path, hint in audit(rs):
            findings.append((kind, tested, str(path.relative_to(root)), m, hint))

    uncalled = [f for f in findings if f[0] == "UNCALLED"]
    subcmd = [f for f in findings if f[0] == "subcommand-only"]

    print(f"unhosted_check: {len(uncalled)} uncalled, {len(subcmd)} subcommand-only")
    for kind, tested, path, m, hint in sorted(uncalled):
        mark = "tested" if tested else "untested"
        extra = f"   <- {hint}" if hint else ""
        print(f"  UNCALLED  {m:22s} [{mark}]  {path}{extra}")
    for kind, tested, path, m, _h in sorted(subcmd):
        mark = "tested" if tested else "untested"
        print(f"  cli-only  {m:22s} [{mark}]  {path}")
    if uncalled or subcmd:
        print()
        print("  tested + uncalled is usually work that stopped one step early.")
        print("  untested + uncalled is usually dead code.")
        print("  cli-only may be intentional (headless sims are how this repo tests).")
        if any(f[4] for f in uncalled):
            print("  '<- x() also in y' means a same-named function is live elsewhere:")
            print("  the SUPERSEDED case, and the only one of the four a tool can spot.")
    return 0  # never fails a build: see the module docstring


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
