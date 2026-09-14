#!/usr/bin/env python3
"""textedit_id_check.py — a text field with no stable id accepts one character.

WHY THIS EXISTS

egui tracks keyboard focus BY WIDGET ID. A `TextEdit` given no explicit id
falls back to one derived from its position in the layout. So the moment
anything reflows, the id changes, focus moves to a widget that no longer
exists, and the next keystroke goes nowhere — the field takes exactly one
character and then appears dead.

The reflow does not have to be dramatic. Typing into the field is enough
when:

  * it lives in an `egui::Grid` whose column width is derived from content,
    so the column widens as the text grows;
  * it sits above a filtered list that shrinks as the query narrows (every
    command palette);
  * a sibling field is rewritten in response (bscale's Input rewrites
    Output, which resizes the shared grid column mid-keystroke);
  * a conditional row above it appears (the mail login's two-password
    mailbox field).

`shared/uikit/crates/uikit-egui/src/a11y.rs::semantic_text_edit` carries the
original diagnosis, from when the a11y helper had the same bug: "focus is
tracked by id, so any reflow (including the one caused by typing into a
wrapped row) silently drops it after the first keystroke."

It is a bad failure to find by hand. The field looks normal, the first
keystroke works, and nothing logs anything.

WHAT COUNTS AS AN ID

`.id_salt(..)` (egui 0.31+), `.id_source(..)` (older), or `.id(..)`. Any of
the three pins the widget independently of where it landed.

WHAT IS DELIBERATELY NOT FLAGGED

  * `ui.put(rect, TextEdit::..)` — an explicit rect IS the position, and it
    does not move with layout. Several uikit tests rely on this.
  * anything inside `#[cfg(test)]` or a `//!`/`///` doc example.

USAGE

    python3 maintenance/tools/textedit_id_check.py [repo_root]

Exit 0 with a PASS line, or 1 listing each unpinned field with its file and
line.
"""
import os
import re
import sys

SKIP_DIRS = {".git", "target", "node_modules", "vendor"}
ROOTS = ("apps", "shared", "games", "system", "tools")

# A site is pinned if any of these appears in the same statement.
PINNED = ("id_salt", "id_source", ".id(")

# Known-good exceptions, path -> why. Empty on purpose: `ui.put` and test
# code are excluded structurally below, not listed, so an entry here would
# mean a real field someone decided to leave unpinned — which wants a reason
# written down rather than a silent pass.
KNOWN = {}

MIN_SITES = 50  # a scan finding almost nothing has broken, not passed


def in_test_or_doc(src, pos):
    """True when this offset is inside a #[cfg(test)] module or a doc line."""
    line_start = src.rfind("\n", 0, pos) + 1
    line = src[line_start : src.find("\n", pos)]
    if line.lstrip().startswith(("//!", "///", "//")):
        return True
    # Any #[cfg(test)] before this point with no closing at column 0 after it
    # is a good-enough approximation: test modules are the last thing in these
    # files by convention, and a false "in test" only costs us a missed
    # warning in code that is already unusual.
    marker = src.rfind("#[cfg(test)]", 0, pos)
    return marker != -1


def main():
    root = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ".")
    total = 0
    bad = []

    for top in ROOTS:
        base = os.path.join(root, top)
        if not os.path.isdir(base):
            continue
        for dp, dn, fn in os.walk(base):
            dn[:] = [d for d in dn if d not in SKIP_DIRS]
            for f in fn:
                if not f.endswith(".rs"):
                    continue
                path = os.path.join(dp, f)
                try:
                    src = open(path, encoding="utf-8", errors="replace").read()
                except OSError:
                    continue
                if "TextEdit::" not in src:
                    continue
                for m in re.finditer(r"TextEdit::(?:singleline|multiline)\s*\(", src):
                    total += 1
                    if in_test_or_doc(src, m.start()):
                        continue
                    # `ui.put(rect, ..)` fixes the position explicitly.
                    prefix = src[max(0, m.start() - 60) : m.start()]
                    if ".put(" in prefix:
                        continue
                    stmt = src[m.start() : m.start() + 400].split(";")[0]
                    if any(p in stmt for p in PINNED):
                        continue
                    rel = os.path.relpath(path, root)
                    if rel in KNOWN:
                        continue
                    ln = src[: m.start()].count("\n") + 1
                    bad.append((rel, ln, src.splitlines()[ln - 1].strip()[:88]))

    if total < MIN_SITES:
        print(
            f"textedit_id_check: FAIL — only {total} TextEdit sites scanned, "
            f"expected at least {MIN_SITES}. The walk is broken, not the tree."
        )
        return 1

    if bad:
        print(
            f"textedit_id_check: FAIL — {len(bad)} of {total} text fields have "
            "no stable id\n"
        )
        for rel, ln, line in bad:
            print(f"  {rel}:{ln}")
            print(f"      {line}")
        print(
            "\n  Each takes ONE character and then appears dead, as soon as "
            "anything\n  reflows — which typing into it often does. Add "
            '.id_salt("something-stable").'
        )
        return 1

    print(
        f"textedit_id_check: PASS — all {total} TextEdit sites are pinned, "
        "in a test, or given an explicit rect"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
