#!/usr/bin/env python3
"""testonly_check — public functions whose only callers are tests.

unhosted_check finds uncalled MODULES. This finds the finer version and
the one that kept getting past it: a public function inside a module
that IS hosted, written and tested, with no caller outside its own test
block.

The snapclone DHT chain is why this exists. Six pieces in a row were
complete, tested, and unreachable — a combo the ids collided on, a
setting nothing read, a bridge with no caller, a Transport that was only
ever simulated, no component owning a socket, and finally
`bootstrap_addresses`, which computed the right list and was called by
three tests and nothing else. unhosted_check saw none of them, because
every one lived in a module that was reachable.

## RUN IT AT THE WIDEST SCOPE THAT COMPILES TOGETHER

A caller in another crate is still a caller. Scanning one crate at a time
reports every cross-crate call as absent, and the numbers are not close:
logscope's four crates scanned separately give 22 findings; scanned
together they give 9. Thirteen of those 22 are live functions, and
someone tidying from the per-crate list would delete working code —
`engmode::from_traces` is called by logscope-egui's `phase_guide` and
looks dead when logscope-fan is scanned alone.

That mattered because the tool used to be too slow to run any other way:
the file READ was cached and the `#[cfg(test)]` span scan was not, so
`test_spans` ran once per (candidate function x file) and the cost was
quadratic in the tree. Caching it took the four-crate run from
"not in seven minutes" to under five. The cost was quietly deciding the
scope, and the scope was deciding the answer.

    python3 testonly_check.py apps/logscope/crates      # yes
    python3 testonly_check.py apps/logscope/crates/foo  # only to triage one crate

## What it reports, and what it deliberately does not

Reported: `pub fn` in a non-test module, referenced by name only inside
`#[cfg(test)]` blocks or `tests/`.

NOT reported:
  * anything in a crate that is a library by intent — a `pub fn` in a
    shared crate has callers in other repos by design, and flagging
    those buries the real finds. Controlled by `--lib-ok`.
  * trait methods and impls, which are called through the trait.
  * `new`, `default`, and other constructors, which are called wherever
    the type is.

The false-positive lesson from unhosted_check applies doubly here: a
checker that is wrong about most of what it reports teaches people to
ignore it, and this one looks at a much larger surface.
"""

import re
import sys
from pathlib import Path

# A pub fn's name, at any indentation.
PUB_FN = re.compile(r"^\s*pub(?:\([^)]*\))?\s+(?:async\s+)?fn\s+([a-z_][a-z0-9_]*)", re.M)

# Names that are called through a type or trait rather than by name, so
# "no direct caller" says nothing about whether they are used.
EXEMPT = {
    "new", "default", "from", "into", "clone", "fmt", "drop",
    "next", "poll", "len", "is_empty", "iter", "eq", "cmp", "hash",
}


def test_spans(text):
    """Byte ranges covered by #[cfg(test)] modules.

    Brace-counted rather than regexed to the end of file: a test module
    is followed by more code often enough that taking everything after
    it would hide every later function.
    """
    spans = []
    for m in re.finditer(r"#\[cfg\(test\)\]", text):
        i = text.find("{", m.end())
        if i < 0:
            continue
        depth, j = 0, i
        while j < len(text):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        spans.append((m.start(), j))
    return spans


def in_span(pos, spans):
    return any(a <= pos <= b for a, b in spans)


def audit(root: Path, lib_ok):
    rs = [
        p for p in root.rglob("*.rs")
        if "/target/" not in str(p) and not p.name.startswith(".")
    ]
    # One pass to read everything: these trees are large and re-reading a
    # file per candidate turned a 20s run into minutes.
    text = {p: p.read_text(errors="replace") for p in rs}
    # ...and one pass to find the test blocks, for the same reason one level in.
    #
    # The read was cached and the SPAN SCAN was not: `test_spans(other)` ran once per
    # (candidate function x file), each time re-scanning a whole file for `#[cfg(test)]`
    # and brace-matching it. That is quadratic in the tree, which is why this tool was
    # only ever run one crate at a time -- and running it per crate is what makes it
    # report cross-crate callers as absent. `engmode::from_traces` is called by
    # logscope-egui's phase_guide and shows up TEST-ONLY when logscope-fan is scanned
    # alone. The cost was quietly deciding the scope, and the scope was deciding the
    # answer.
    spans_of = {p: test_spans(t) for p, t in text.items()}
    findings = []

    for p in rs:
        # A file under tests/ defines test helpers by definition.
        if "/tests/" in str(p) or p.name.startswith("test_"):
            continue
        if any(str(p).startswith(str(root / d)) for d in lib_ok):
            continue
        body = text[p]
        spans = spans_of[p]
        for m in PUB_FN.finditer(body):
            name = m.group(1)
            if name in EXEMPT or in_span(m.start(), spans):
                continue
            # Called anywhere outside a test block, in any file?
            hosted = False
            call = re.compile(r"\b" + re.escape(name) + r"\s*\(")
            for q, other in text.items():
                if hosted:
                    break
                ospans = spans_of[q]
                for c in call.finditer(other):
                    # The definition itself is not a call.
                    if q == p and abs(c.start() - m.end()) < 4:
                        continue
                    if not in_span(c.start(), ospans):
                        hosted = True
                        break
            if not hosted:
                line = body[: m.start()].count("\n") + 1
                findings.append((p.relative_to(root), line, name))
    return findings


def main(argv):
    root = Path(argv[1] if len(argv) > 1 else ".")
    # Shared crates are libraries by intent; their callers are elsewhere.
    lib_ok = ["shared"]
    for a in argv[2:]:
        if a.startswith("--lib-ok="):
            lib_ok = [x for x in a.split("=", 1)[1].split(",") if x]
    found = audit(root, lib_ok)
    print(f"testonly_check: {len(found)} pub fn(s) called only by tests")
    for path, line, name in sorted(found):
        print(f"  TEST-ONLY  {name:<28} {path}:{line}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
