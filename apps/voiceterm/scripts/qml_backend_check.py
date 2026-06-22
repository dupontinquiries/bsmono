#!/usr/bin/env python3
"""
qml_backend_check.py — verify every `backend.<name>` referenced in QML
corresponds to a real qt_method / qt_property / qt_signal declared in
src/main.rs.

This catches the one failure mode qml_lint.py CANNOT: a QML call to a
backend member that doesn't exist (typo, renamed, removed). Because
QML method calls resolve at runtime, such a call fails silently — and
when it's behind a `if (backend && backend.foo)` guard, the guard is
ALWAYS FALSE, so the feature just quietly does nothing.

Found exactly that on first run (v0.1.502): QML called
`backend.set_clipboard(...)` but the method is `clipboard_set`, so the
"copy install hint" button silently did nothing.

False-positive avoidance (learned from the prototype):
  - strips QML // line and /* */ block comments before scanning (the
    prototype flagged example usages inside doc-comments);
  - matches qt_method/qt_property/qt_signal even when several are
    declared on a single source line (the prototype's line-anchored
    regex missed `x: qt_property!(..), y: qt_signal!()`).
  - treats every property as having derived `<p>_changed`/`<p>Changed`
    NOTIFY signal names (qmetaobject convention).

Run from repo root:
    python3 scripts/qml_backend_check.py
Exit code: 0 if no findings, 1 otherwise.
"""

import re
import sys
from pathlib import Path


def strip_qml_comments(src: str) -> str:
    """Replace // line and /* */ block comments with spaces, preserving
    newlines so line numbers stay meaningful. String-literal aware
    enough for our purpose (backend.<name> never appears in a string we
    care about)."""
    out = []
    i = 0
    n = len(src)
    in_lc = in_bc = False
    in_str = None
    while i < n:
        c = src[i]
        nxt = src[i + 1] if i + 1 < n else ''
        if in_lc:
            if c == '\n':
                in_lc = False
                out.append(c)
            else:
                out.append(' ')
            i += 1
        elif in_bc:
            if c == '*' and nxt == '/':
                in_bc = False
                out.append('  ')
                i += 2
            else:
                out.append('\n' if c == '\n' else ' ')
                i += 1
        elif in_str:
            out.append(c)
            if c == '\\' and nxt:
                out.append(nxt)
                i += 2
                continue
            if c == in_str:
                in_str = None
            i += 1
        else:
            if c == '/' and nxt == '/':
                in_lc = True
                out.append('  ')
                i += 2
            elif c == '/' and nxt == '*':
                in_bc = True
                out.append('  ')
                i += 2
            elif c in ('"', "'", '`'):
                in_str = c
                out.append(c)
                i += 1
            else:
                out.append(c)
                i += 1
    return ''.join(out)


def main() -> int:
    root = Path(".")
    rs = (root / "src" / "main.rs").read_text()

    # Match anywhere on a line (several decls can share one line).
    methods = set(re.findall(r'([a-z_][a-z0-9_]*)\s*:\s*qt_method!', rs))
    props = set(re.findall(r'([a-z_][a-z0-9_]*)\s*:\s*qt_property!', rs))
    signals = set(re.findall(r'([a-z_][a-z0-9_]*)\s*:\s*qt_signal!', rs))

    valid = set(methods) | set(props) | set(signals)
    for p in props:
        valid.add(p + "_changed")
        valid.add(p + "Changed")

    qml_refs = {}
    for qf in sorted((root / "qml").glob("*.qml")):
        txt = strip_qml_comments(qf.read_text())
        for m in re.finditer(r'backend\.([a-z_][a-z0-9_]*)', txt):
            qml_refs.setdefault(m.group(1), set()).add(qf.name)

    missing = sorted(set(qml_refs) - valid)

    print(f"declared: {len(methods)} methods, {len(props)} props, "
          f"{len(signals)} signals ({len(valid)} valid names incl. derived)")
    print(f"QML backend.<x> unique refs: {len(qml_refs)}")
    if not missing:
        print("qml_backend_check: clean — every backend ref resolves")
        return 0
    print(f"qml_backend_check: {len(missing)} UNRESOLVED backend ref(s):")
    for nm in missing:
        where = ", ".join(sorted(qml_refs[nm]))
        print(f"  MISSING: backend.{nm}  (in {where})")
    return 1


if __name__ == "__main__":
    sys.exit(main())
